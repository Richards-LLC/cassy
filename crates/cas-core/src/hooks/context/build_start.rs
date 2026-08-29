use crate::error::CoreError;
use crate::hooks::config::HooksConfig;
use crate::hooks::context::{
    BasicContextScorer, ContextItem, ContextQuery, ContextScorer, ContextStats, ContextStores,
    SurfacedItemCallback, USAGE_REMINDER, estimate_tokens, format_category_badge, merge_entries,
    merge_rules, merge_skills, remap_tool_prefix, render_factory_coordination,
    render_normal_coordination, rule_matches_path, token_display,
};
use crate::hooks::types::HookInput;
use crate::memory::{is_high_importance_preference, session_memory_preview};
use crate::truncate;
use cas_store::KnowledgeStore;
use cas_types::{
    AgentRole, AgentStatus, Entry, EntryType, Rule, RuleStatus, Scope, Skill, SkillStatus,
    TaskStatus,
};
use std::collections::HashSet;

/// Token ceiling for the distilled-knowledge index section.
///
/// The index is a *pointer list*, not content: it must stay small enough that
/// it never crowds out tasks, rules or memories no matter how many pages the
/// distillation pipeline has produced.
const KNOWLEDGE_SECTION_TOKEN_BUDGET: usize = 600;

/// Longest snippet rendered per page before truncation.
const KNOWLEDGE_SNIPPET_CHARS: usize = 120;

/// Persist one impact sample for a rule that made it into the injected
/// context. Store failures are intentionally best-effort: a metrics write
/// must not make SessionStart fail after the context itself was built.
fn increment_rule_surface_count(stores: &ContextStores, rule: &Rule) {
    let store = match rule.scope {
        Scope::Global => stores.global_rule_store.or(stores.project_rule_store),
        Scope::Project => stores.project_rule_store.or(stores.global_rule_store),
    };
    if let Some(store) = store {
        let _ = store.increment_surface_count(&rule.id);
    }
}

/// Render the distilled-knowledge index: one line per page, no bodies.
///
/// This is the "inject the index, toolize the body" shape. Every line carries
/// the page id, so the agent can pull the full prose on demand through the
/// knowledge MCP tool instead of paying for it in every session.
///
/// Determinism matters as much as size here. The injected block is a prompt-
/// cache prefix, so it must be byte-identical across runs on an unchanged
/// store: pages are ordered by `(page_type, title, id)` — never by a score or
/// a timestamp — and nothing time-derived is rendered. `list_pages` already
/// orders by type then title; the explicit re-sort makes the guarantee local
/// to this function instead of an assumption about the store.
///
/// Returns the section text (empty when there are no pages) and the tokens it
/// consumed.
fn render_knowledge_index(ks: &dyn KnowledgeStore, token_budget: usize) -> (String, usize) {
    let Ok(mut pages) = ks.list_pages() else {
        return (String::new(), 0);
    };
    if pages.is_empty() {
        return (String::new(), 0);
    }
    pages.sort_by(|a, b| {
        a.page_type
            .cmp(&b.page_type)
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.id.cmp(&b.id))
    });

    let total_pages = pages.len();
    let mut lines: Vec<String> = Vec::new();
    let mut used = 0usize;

    for page in &pages {
        let snippet = truncate(&page.snippet.replace('\n', " "), KNOWLEDGE_SNIPPET_CHARS);
        let line = if snippet.is_empty() {
            format!("- {} [{}] {}", page.id, page.page_type, page.title)
        } else {
            format!(
                "- {} [{}] {} — {}",
                page.id, page.page_type, page.title, snippet
            )
        };
        let line_tokens = estimate_tokens(&line) + 4;
        if used + line_tokens > token_budget {
            break;
        }
        used += line_tokens;
        lines.push(line);
    }

    if lines.is_empty() {
        return (String::new(), 0);
    }

    let header = if lines.len() < total_pages {
        format!(
            "## 📚 Project Knowledge ({}/{} pages indexed)",
            lines.len(),
            total_pages
        )
    } else {
        format!("## 📚 Project Knowledge ({total_pages} pages)")
    };

    let mut section = String::with_capacity(header.len() + used * 4);
    section.push_str(&header);
    section.push('\n');
    section.push('\n');
    section.push_str(&lines.join("\n"));
    section.push('\n');
    section.push('\n');
    section.push_str(KNOWLEDGE_PULL_INSTRUCTION);

    used += estimate_tokens(&header) + estimate_tokens(KNOWLEDGE_PULL_INSTRUCTION);
    (section, used)
}

/// The pull half of "index-inject / body-pull".
///
/// Kept as one constant so the MCP surface (T4) can rename its actions without
/// touching the context builder. `mcp__cas__` is remapped to the reader's own
/// prefix by `remap_tool_prefix` at the end of `build_context_with_stores`.
///
/// It is `pub` on purpose: the action name it advertises must stay dispatchable
/// by the real `knowledge` MCP router, which lives in `cas-cli` and cannot be
/// reached from here. `cas-cli`'s `knowledge_tools` suite reads this constant
/// and drives the named action end-to-end, so a rename on either side fails a
/// test instead of silently shipping an index whose pull half 404s.
pub const KNOWLEDGE_PULL_INSTRUCTION: &str = "These are titles only. Read a page body with `mcp__cas__knowledge` (action: read, id) before answering questions about how this project works — do not guess from the title.";

/// Build context string for session start injection
///
/// Uses progressive disclosure pattern:
/// - Shows summaries with token estimates
/// - Full content available via `cas show <id>`
/// - Allows Claude to decide what to fetch based on relevance
///
/// `tool_prefix` is the READER's own MCP tool-call prefix (`mcp__cas__` for
/// Claude, `mcp__cs__` for Codex, `cas__` for Grok — see
/// `harness_policy::own_tool_prefix` in `cas-cli`, the canonical caller-side
/// source of this value). Every `mcp__cas__`-prefixed tool mention this
/// function assembles is remapped to `tool_prefix` exactly once, at the very
/// end, via `remap_tool_prefix` — see that function's doc comment for why a
/// single end-of-function pass replaced the old per-site `input.source ==
/// "codex"` closure (cas-fd9f).
pub fn build_context_with_stores(
    input: &HookInput,
    stores: &ContextStores,
    config: &dyn HooksConfig,
    limit: usize,
    on_surfaced: Option<&SurfacedItemCallback>,
    tool_prefix: &str,
) -> Result<(String, ContextStats), CoreError> {
    let token_budget = config.token_budget();
    let mcp_enabled = config.mcp_enabled();

    let merged_rules = merge_rules(stores);
    let merged_skills = merge_skills(stores);
    let merged_entries = merge_entries(stores);

    let mut context_parts = Vec::new();
    let mut total_tokens: usize = 0;
    let budget_remaining = |used: usize| -> usize {
        if token_budget == 0 {
            usize::MAX
        } else {
            token_budget.saturating_sub(used)
        }
    };

    let mut stats = ContextStats::default();

    // Track available items for "More Context Available" section in minimal mode
    let minimal_start = config.minimal_start();
    let mut available_ready_tasks = 0usize;
    let mut available_ready_tasks_tokens = 0usize;
    let mut available_memories = 0usize;
    let mut available_memories_tokens = 0usize;

    // Add CAS usage reminder at the top
    let usage_reminder = USAGE_REMINDER.trim().to_string();
    total_tokens += estimate_tokens(&usage_reminder);
    context_parts.push(usage_reminder);

    // Add session ID info (auto-registration handles the rest)
    if mcp_enabled && !input.session_id.is_empty() {
        let session_info = format!(
            "\n**Session:** `{}` (auto-registers on first CAS tool use)",
            input.session_id
        );
        context_parts.push(session_info.clone());
        total_tokens += estimate_tokens(&session_info);
    }

    // Check for blocked in-progress tasks (important alert - always show full)
    if let Some(ts) = stores.task_store {
        if let Ok(blocked) = ts.list_blocked() {
            let blocked_in_progress: Vec<_> = blocked
                .iter()
                .filter(|(task, _)| task.status == TaskStatus::InProgress)
                .collect();

            if !blocked_in_progress.is_empty() {
                context_parts.push("## ⚠️ Blocked Tasks (Action Required)".to_string());
                context_parts.push(String::new());
                for (task, blockers) in blocked_in_progress {
                    let blocker_ids: Vec<_> = blockers.iter().map(|b| b.id.as_str()).collect();
                    let line = format!(
                        "- **{}** {} — blocked by: {}",
                        task.id,
                        task.preview(50),
                        blocker_ids.join(", ")
                    );
                    total_tokens += estimate_tokens(&line);
                    context_parts.push(line);
                }
            }
        }
    }

    // Track current agent's claimed task IDs for filtering In Progress section
    let mut current_agent_task_ids: HashSet<String> = HashSet::new();

    // Add agent coordination info (multi-agent awareness)
    if let Some(as_store) = stores.agent_store {
        let current_pid = std::process::id();

        // Get both Active and Idle agents (all alive agents)
        let mut all_agents = Vec::new();
        if let Ok(active) = as_store.list(Some(AgentStatus::Active)) {
            all_agents.extend(active);
        }
        if let Ok(idle) = as_store.list(Some(AgentStatus::Idle)) {
            all_agents.extend(idle);
        }

        // Deduplicate by ID (in case of any overlap)
        let mut seen_ids = HashSet::new();
        all_agents.retain(|a| seen_ids.insert(a.id.clone()));

        // Check if this is factory mode - via database agents OR env var
        // Env var check handles cases where agent isn't registered yet
        let is_factory_via_env = std::env::var("CAS_AGENT_ROLE")
            .map(|r| matches!(r.to_lowercase().as_str(), "worker" | "supervisor"))
            .unwrap_or(false);
        let is_factory_via_db = all_agents
            .iter()
            .any(|a| matches!(a.role, AgentRole::Worker | AgentRole::Supervisor));
        let is_factory_mode = is_factory_via_db || is_factory_via_env;

        // Find current agent by matching PID
        let current_agent = all_agents.iter().find(|a| a.pid == Some(current_pid));

        // Capture current agent's claimed task IDs for later filtering
        if let Some(agent) = current_agent {
            if let Ok(leases) = as_store.list_agent_leases(&agent.id) {
                current_agent_task_ids = leases.iter().map(|l| l.task_id.clone()).collect();
            }
        }

        let other_agents: Vec<_> = all_agents
            .iter()
            .filter(|a| a.pid != Some(current_pid))
            .collect();

        // Show agent section if there's multi-agent activity OR we're in factory mode
        // Factory mode via env var should always show coordination context
        if all_agents.len() > 1 || current_agent.is_some() || is_factory_mode {
            if !context_parts.is_empty() {
                context_parts.push(String::new());
            }
            context_parts.push("## 🤖 Agent Coordination".to_string());
            context_parts.push(String::new());

            if is_factory_mode {
                // Factory-aware format (minimal, role-aware)
                render_factory_coordination(
                    &mut context_parts,
                    &mut total_tokens,
                    current_agent,
                    &other_agents,
                    as_store,
                    stores.task_store,
                    config,
                );
            } else {
                // Normal mode format
                render_normal_coordination(
                    &mut context_parts,
                    &mut total_tokens,
                    current_agent,
                    &other_agents,
                    as_store,
                    stores.task_store,
                );
            }
        }
    }

    // Add pinned memories (in-context tier - always shown full, ignores budget)
    if let Some(store) = stores.primary_store() {
        if let Ok(pinned_entries) = store.list_pinned() {
            let pinned_entries: Vec<_> = pinned_entries
                .into_iter()
                .filter(|entry| !entry.is_expired())
                .collect();
            if !pinned_entries.is_empty() {
                if !context_parts.is_empty() {
                    context_parts.push(String::new());
                }
                context_parts.push("## 📌 Pinned Memories (Always Active)".to_string());
                context_parts.push(String::new());
                for entry in &pinned_entries {
                    let item = ContextItem::from_entry(entry);
                    // Show full content for pinned entries (they're critical context)
                    let title = entry.title.clone().unwrap_or_else(|| entry.preview(60));
                    context_parts.push(format!("### {} [{}]", item.id, entry.entry_type));
                    if !title.is_empty() && title != entry.preview(60) {
                        context_parts.push(format!("**{title}**"));
                    }
                    context_parts.push(String::new());
                    context_parts.push(entry.content.clone());
                    context_parts.push(String::new());
                    total_tokens += item.tokens;
                    stats.pinned_included += 1;
                }
            }
        }
    }

    // In minimal_start mode, stop here - only blocked tasks and pinned memories
    if minimal_start {
        context_parts.push(String::new());
        context_parts.push(format!(
            "**Context: ~{total_tokens}tk (minimal mode)** | Use `cas context` for full context | Search: `cas search \"<query>\"`"
        ));
        stats.total_tokens = total_tokens;
        return Ok((
            remap_tool_prefix(&context_parts.join("\n"), tool_prefix),
            stats,
        ));
    }

    // Show ALL in-progress tasks
    if let Some(ts) = stores.task_store {
        if let Ok(tasks) = ts.list(Some(TaskStatus::InProgress)) {
            let in_progress: Vec<_> = tasks.iter().take(5).collect();
            if !in_progress.is_empty() {
                if !context_parts.is_empty() {
                    context_parts.push(String::new());
                }
                context_parts.push("## In Progress".to_string());
                context_parts.push(String::new());
                for task in in_progress {
                    let item = ContextItem::from_task(task);
                    let is_mine = current_agent_task_ids.contains(&task.id);
                    let marker = if is_mine { "▶ " } else { "" };
                    let line = format!(
                        "- {}**{}** {} ({}) [{}]",
                        marker,
                        item.id,
                        item.summary,
                        task.priority.label(),
                        token_display(item.tokens)
                    );
                    total_tokens += estimate_tokens(&line);
                    context_parts.push(line);
                    stats.tasks_included += 1;
                }
            }
        }
    }

    // Add ready tasks (progressive disclosure - summaries only)
    // Skip in minimal mode but track availability
    if let Some(ts) = stores.task_store {
        if let Ok(ready_tasks) = ts.list_ready() {
            let all_tasks: Vec<_> = ready_tasks
                .iter()
                .filter(|t| t.status != TaskStatus::InProgress)
                .collect();

            // Track availability for minimal mode summary
            available_ready_tasks = all_tasks.len();
            available_ready_tasks_tokens = all_tasks
                .iter()
                .take(limit)
                .map(|t| ContextItem::from_task(t).tokens)
                .sum();

            // Only show ready tasks in non-minimal mode
            if !minimal_start && budget_remaining(total_tokens) > 100 {
                let mut tasks_to_show = Vec::new();
                let mut section_tokens = 0;
                for task in all_tasks.iter().take(limit) {
                    let item_tokens = estimate_tokens(&task.preview(60)) + 30;
                    if section_tokens + item_tokens < budget_remaining(total_tokens) - 50 {
                        tasks_to_show.push(*task);
                        section_tokens += item_tokens;
                    } else {
                        break;
                    }
                }

                if !tasks_to_show.is_empty() {
                    if !context_parts.is_empty() {
                        context_parts.push(String::new());
                    }
                    let full_section_tokens: usize = all_tasks
                        .iter()
                        .take(limit)
                        .map(|t| ContextItem::from_task(t).tokens)
                        .sum();
                    let omitted = all_tasks.len().saturating_sub(tasks_to_show.len());
                    let header = if omitted > 0 {
                        format!(
                            "## Ready Tasks ({}/{} shown, {} if all expanded)",
                            tasks_to_show.len(),
                            all_tasks.len().min(limit),
                            token_display(full_section_tokens)
                        )
                    } else {
                        format!(
                            "## Ready Tasks ({} tasks, {} if expanded)",
                            tasks_to_show.len(),
                            token_display(full_section_tokens)
                        )
                    };
                    context_parts.push(header);
                    context_parts.push(String::new());
                    for task in &tasks_to_show {
                        let item = ContextItem::from_task(task);
                        context_parts.push(format!(
                            "- {} {} ({}) [{}]",
                            item.id,
                            item.summary,
                            task.priority.label(),
                            token_display(item.tokens)
                        ));
                        total_tokens += estimate_tokens(&item.summary) + 20;
                        stats.tasks_included += 1;
                    }
                    stats.items_omitted += omitted;
                }
            }
        }
    }

    // Add proven rules with priority-based ordering
    {
        // Use cache if available, otherwise fall back to direct matching
        let matches_cwd = |rule: &Rule| -> bool {
            if let Some(cache) = stores.rule_match_cache {
                cache.matches(rule, &input.cwd)
            } else {
                rule_matches_path(rule, &input.cwd)
            }
        };

        let mut all_proven: Vec<_> = merged_rules
            .iter()
            .filter(|r| r.status == RuleStatus::Proven)
            .filter(|r| matches_cwd(r))
            .collect();

        all_proven.sort_by_key(|r| r.priority);

        // Critical rules always shown
        let critical_rules: Vec<_> = all_proven
            .iter()
            .filter(|r| r.is_critical())
            .copied()
            .collect();

        if !critical_rules.is_empty() {
            if !context_parts.is_empty() {
                context_parts.push(String::new());
            }
            context_parts.push("## ⚠️ Critical Rules (Always Active)".to_string());
            context_parts.push(String::new());
            for rule in &critical_rules {
                let item = ContextItem::from_rule(rule);
                let category_badge = format_category_badge(rule.category);
                context_parts.push(format!(
                    "- [{}] {} {}",
                    category_badge, item.id, rule.content
                ));
                total_tokens += item.tokens;
                stats.rules_included += 1;

                increment_rule_surface_count(stores, rule);
                if let Some(callback) = on_surfaced {
                    callback(&item.id, "rule", Some(&item.summary));
                }
            }
        }

        // Regular rules with budget management
        if budget_remaining(total_tokens) > 100 {
            let regular_rules: Vec<_> = all_proven
                .iter()
                .filter(|r| !r.is_critical())
                .copied()
                .collect();

            let mut rules_to_show = Vec::new();
            let mut section_tokens = 0;
            for rule in regular_rules.iter().take(limit) {
                let item = ContextItem::from_rule(rule);
                let rule_tokens = if item.tokens < 50 {
                    item.tokens
                } else {
                    estimate_tokens(&item.summary) + 20
                };
                if section_tokens + rule_tokens < budget_remaining(total_tokens) - 50 {
                    rules_to_show.push(*rule);
                    section_tokens += rule_tokens;
                } else {
                    break;
                }
            }

            if !rules_to_show.is_empty() {
                if !context_parts.is_empty() {
                    context_parts.push(String::new());
                }
                let full_section_tokens: usize = regular_rules
                    .iter()
                    .take(limit)
                    .map(|r| ContextItem::from_rule(r).tokens)
                    .sum();
                let omitted = regular_rules
                    .len()
                    .min(limit)
                    .saturating_sub(rules_to_show.len());
                let header = if omitted > 0 {
                    format!(
                        "## Active Rules ({}/{} shown, {} total)",
                        rules_to_show.len(),
                        regular_rules.len().min(limit),
                        token_display(full_section_tokens)
                    )
                } else {
                    format!(
                        "## Active Rules ({} rules, {} total)",
                        rules_to_show.len(),
                        token_display(full_section_tokens)
                    )
                };
                context_parts.push(header);
                context_parts.push(String::new());
                for rule in &rules_to_show {
                    let item = ContextItem::from_rule(rule);
                    let category_badge = format_category_badge(rule.category);
                    if item.tokens < 50 {
                        context_parts.push(format!(
                            "- [{}] {} {}",
                            category_badge, item.id, rule.content
                        ));
                        total_tokens += item.tokens;
                    } else {
                        context_parts.push(format!(
                            "- [{}] {} {} [{}]",
                            category_badge,
                            item.id,
                            item.summary,
                            token_display(item.tokens)
                        ));
                        total_tokens += estimate_tokens(&item.summary) + 20;
                    }
                    stats.rules_included += 1;

                    increment_rule_surface_count(stores, rule);
                    if let Some(callback) = on_surfaced {
                        callback(&item.id, "rule", Some(&item.summary));
                    }
                }
                stats.items_omitted += regular_rules
                    .len()
                    .min(limit)
                    .saturating_sub(rules_to_show.len());
            }
        }
    }

    // Add enabled skills
    // Skip in minimal mode but track availability
    let (available_skills, available_skills_tokens) = {
        let mut enabled_skills: Vec<&Skill> = merged_skills
            .iter()
            .filter(|s| s.status == SkillStatus::Enabled)
            .collect();
        enabled_skills.sort_by(|a, b| b.usage_count.cmp(&a.usage_count));

        // Track availability for minimal mode summary
        let available_skills = enabled_skills.len();
        let available_skills_tokens = enabled_skills
            .iter()
            .take(limit)
            .map(|s| ContextItem::from_skill(s).tokens)
            .sum();

        // Only show skills in non-minimal mode
        if !minimal_start && budget_remaining(total_tokens) > 100 {
            let mut skills_to_show: Vec<&Skill> = Vec::new();
            let mut section_tokens = 0;
            for skill in enabled_skills.iter().take(limit) {
                let item_tokens = estimate_tokens(&skill.description) + 30;
                if section_tokens + item_tokens < budget_remaining(total_tokens) - 50 {
                    skills_to_show.push(skill);
                    section_tokens += item_tokens;
                } else {
                    break;
                }
            }

            if !skills_to_show.is_empty() {
                if !context_parts.is_empty() {
                    context_parts.push(String::new());
                }
                let full_section_tokens: usize = enabled_skills
                    .iter()
                    .take(limit)
                    .map(|s| ContextItem::from_skill(s).tokens)
                    .sum();
                let omitted = enabled_skills
                    .len()
                    .min(limit)
                    .saturating_sub(skills_to_show.len());
                let header = if omitted > 0 {
                    format!(
                        "## Available Skills ({}/{} shown, {} if expanded)",
                        skills_to_show.len(),
                        enabled_skills.len().min(limit),
                        token_display(full_section_tokens)
                    )
                } else {
                    format!(
                        "## Available Skills ({} skills, {} if expanded)",
                        skills_to_show.len(),
                        token_display(full_section_tokens)
                    )
                };
                context_parts.push(header);
                context_parts.push(String::new());
                for skill in &skills_to_show {
                    let item = ContextItem::from_skill(skill);
                    let usage = if skill.usage_count > 0 {
                        format!(" ({}x)", skill.usage_count)
                    } else {
                        String::new()
                    };
                    context_parts.push(format!(
                        "- {} `{}`: {}{}",
                        item.id, skill.name, item.summary, usage
                    ));
                    total_tokens += estimate_tokens(&item.summary) + 20;
                    stats.skills_included += 1;
                }
                stats.items_omitted += enabled_skills
                    .len()
                    .min(limit)
                    .saturating_sub(skills_to_show.len());
            }
        }

        (available_skills, available_skills_tokens)
    };

    // Add valuable entries using scorer (hybrid or basic)
    // Skip display in minimal mode but track availability
    if budget_remaining(total_tokens) > 100 {
        // Build context query for semantic scoring
        let mut task_titles = Vec::new();
        if let Some(ts) = stores.task_store {
            if let Ok(in_progress) = ts.list(Some(TaskStatus::InProgress)) {
                task_titles = in_progress.iter().map(|t| t.title.clone()).collect();
            }
        }
        let context_query = ContextQuery {
            task_titles,
            cwd: input.cwd.clone(),
            user_prompt: input.user_prompt.clone(),
            recent_files: stores.recent_files.clone(),
        };

        // Filter entries first
        let filtered_entries: Vec<Entry> = merged_entries
            .iter()
            .filter(|e| e.entry_type != EntryType::Observation || e.feedback_score() > 0)
            .filter(|e| !e.is_expired())
            .cloned()
            .collect();

        // Use provided scorer or fallback to basic
        let basic_scorer = BasicContextScorer;
        let scorer: &dyn ContextScorer = stores.entry_scorer.unwrap_or(&basic_scorer);

        // Score entries using the scorer
        let mut scored_entries = scorer.score_entries(&filtered_entries, &context_query);
        // Standing preferences at the high-importance threshold are operative
        // constraints. Keep them ahead of compact learnings when the memory
        // section has to omit entries for the hook budget.
        scored_entries.sort_by(|(a, a_score), (b, b_score)| {
            is_high_importance_preference(b)
                .cmp(&is_high_importance_preference(a))
                .then_with(|| {
                    b_score
                        .partial_cmp(a_score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                })
        });

        // Track availability for minimal mode summary
        available_memories = scored_entries.len().min(limit);
        available_memories_tokens = scored_entries
            .iter()
            .take(limit)
            .map(|(e, _)| ContextItem::from_entry(e).tokens)
            .sum();

        let mut entries_to_show: Vec<&Entry> = Vec::new();
        let mut section_tokens = 0;
        for (entry, _score) in scored_entries.iter().take(limit) {
            let item_tokens = estimate_tokens(&session_memory_preview(entry)) + 30;
            if section_tokens + item_tokens < budget_remaining(total_tokens) - 50 {
                entries_to_show.push(entry);
                section_tokens += item_tokens;
            } else {
                break;
            }
        }

        // Only show memories in non-minimal mode
        if !minimal_start && !entries_to_show.is_empty() {
            if !context_parts.is_empty() {
                context_parts.push(String::new());
            }
            let total_available = scored_entries.len().min(limit);
            let full_section_tokens: usize = scored_entries
                .iter()
                .take(limit)
                .map(|(e, _)| ContextItem::from_entry(e).tokens)
                .sum();
            let omitted = total_available.saturating_sub(entries_to_show.len());
            let header = if omitted > 0 {
                format!(
                    "## Helpful Memories ({}/{} shown, +{} more, {} if expanded)",
                    entries_to_show.len(),
                    total_available,
                    omitted,
                    token_display(full_section_tokens)
                )
            } else {
                format!(
                    "## Helpful Memories ({} memories, {} if expanded)",
                    entries_to_show.len(),
                    token_display(full_section_tokens)
                )
            };
            context_parts.push(header);
            context_parts.push(String::new());
            for entry in &entries_to_show {
                let item = ContextItem::from_entry(entry);
                let feedback = entry.feedback_score();
                let score_display = if feedback > 0 {
                    format!("(+{feedback})")
                } else if feedback < 0 {
                    format!("({feedback})")
                } else {
                    String::new()
                };
                context_parts.push(format!(
                    "- {} [{}] {} {} [{}]",
                    item.id,
                    entry.entry_type,
                    item.summary,
                    score_display,
                    token_display(item.tokens)
                ));
                total_tokens += estimate_tokens(&item.summary) + 25;
                stats.memories_included += 1;

                if let Some(callback) = on_surfaced {
                    callback(
                        &item.id,
                        &format!("{:?}", item.item_type),
                        Some(&item.summary),
                    );
                }
            }
            stats.items_omitted += omitted;
        }

        // Add the distilled-knowledge index (index-inject / body-pull)
        if !minimal_start && budget_remaining(total_tokens) > 100 {
            if let Some(ks) = stores.knowledge_store {
                let (section, used) = render_knowledge_index(
                    ks,
                    budget_remaining(total_tokens)
                        .min(KNOWLEDGE_SECTION_TOKEN_BUDGET)
                        .saturating_sub(50),
                );
                if !section.is_empty() {
                    if !context_parts.is_empty() {
                        context_parts.push(String::new());
                    }
                    context_parts.push(section);
                    total_tokens += used;
                }
            }
        }

        // Add "Related to Current Work" section using semantic search
        // Only shown if we have a scorer with semantic capability and meaningful context query
        // Skip in minimal mode
        if !minimal_start
            && budget_remaining(total_tokens) > 100
            && context_query.has_content()
            && scorer.name() == "hybrid"
        {
            // Reuse scored entries from above — no need to re-score
            // Filter to entries not already shown in Helpful Memories
            let shown_ids: HashSet<_> = entries_to_show.iter().map(|e| e.id.as_str()).collect();
            let related_new: Vec<_> = scored_entries
                .iter()
                .filter(|(e, score)| !shown_ids.contains(e.id.as_str()) && *score > 0.3)
                .take(5)
                .collect();

            if !related_new.is_empty() {
                if !context_parts.is_empty() {
                    context_parts.push(String::new());
                }

                let related_section_tokens: usize = related_new
                    .iter()
                    .map(|(e, _)| ContextItem::from_entry(e).tokens)
                    .sum();

                context_parts.push(format!(
                    "## Related to Current Work ({} items, {} if expanded)",
                    related_new.len(),
                    token_display(related_section_tokens)
                ));
                context_parts.push(String::new());

                for (entry, _score) in &related_new {
                    let item = ContextItem::from_entry(entry);
                    context_parts.push(format!(
                        "- {} [{}] {}  [{}]",
                        item.id,
                        entry.entry_type,
                        item.summary,
                        token_display(item.tokens)
                    ));
                    total_tokens += estimate_tokens(&item.summary) + 20;

                    if let Some(callback) = on_surfaced {
                        callback(&item.id, "related", Some(&item.summary));
                    }
                }
            }
        }
    }

    // In minimal mode, show "More Context Available" summary
    if minimal_start {
        let has_available =
            available_ready_tasks > 0 || available_memories > 0 || available_skills > 0;
        if has_available {
            if !context_parts.is_empty() {
                context_parts.push(String::new());
            }
            context_parts.push("## 📦 More Context Available".to_string());
            context_parts.push(String::new());

            let fetch_cmd = "`mcp__cas__search` or `mcp__cas__task`";

            if available_ready_tasks > 0 {
                context_parts.push(format!(
                    "- {} ready tasks ({})",
                    available_ready_tasks,
                    token_display(available_ready_tasks_tokens)
                ));
            }
            if available_memories > 0 {
                context_parts.push(format!(
                    "- {} relevant memories ({})",
                    available_memories,
                    token_display(available_memories_tokens)
                ));
            }
            if available_skills > 0 {
                context_parts.push(format!(
                    "- {} available skills ({})",
                    available_skills,
                    token_display(available_skills_tokens)
                ));
            }

            context_parts.push(String::new());
            context_parts.push(format!("Use {fetch_cmd} to fetch more context as needed."));
        }
    }

    // Add usage hints with token summary and budget info
    if !context_parts.is_empty() {
        context_parts.push(String::new());
        let budget_info = if token_budget > 0 {
            format!(" (budget: {})", token_display(token_budget))
        } else {
            String::new()
        };
        let hints = "Fetch details: `mcp__cas__memory` | Search: `mcp__cas__search`";
        context_parts.push(format!(
            "**Context: {}{}** | {}",
            token_display(total_tokens),
            budget_info,
            hints
        ));
    }

    stats.total_tokens = total_tokens;
    Ok((
        remap_tool_prefix(&context_parts.join("\n"), tool_prefix),
        stats,
    ))
}
