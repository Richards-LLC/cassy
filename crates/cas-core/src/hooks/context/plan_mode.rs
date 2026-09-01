use crate::error::CoreError;
use crate::hooks::config::HooksConfig;
use crate::hooks::context::{
    ContextItem, ContextStats, ContextStores, estimate_tokens, merge_rules, remap_tool_prefix,
    rule_matches_path, token_display, truncate,
};
use crate::hooks::types::HookInput;
use cas_store::TaskStore;
use cas_types::{Dependency, Rule, RuleStatus, Task, TaskStatus};
use std::collections::HashSet;

/// `tool_prefix` is the reader's own MCP tool-call prefix (`mcp__cas__` /
/// `mcp__cs__` / `cas__` — see `harness_policy::own_tool_prefix` in
/// `cas-cli`). The "Planning Tools" hints below are authored against the
/// Claude `mcp__cas__` baseline and remapped once via `remap_tool_prefix`
/// right before returning (cas-fd9f — this function previously had no remap
/// at all, so a non-Claude reader always saw Claude's tool names here).
pub fn build_plan_context_with_stores(
    input: &HookInput,
    stores: &ContextStores,
    config: &dyn HooksConfig,
    _limit: usize,
    tool_prefix: &str,
) -> Result<(String, ContextStats), CoreError> {
    let plan_config = config.plan_mode();
    let token_budget = plan_config.token_budget;
    let task_limit = plan_config.task_limit;

    let merged_rules = merge_rules(stores);

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

    // Header indicating plan mode
    context_parts.push("# 📋 Plan Mode Context".to_string());
    context_parts.push(String::new());
    context_parts.push("*This context is optimized for implementation planning.*".to_string());
    context_parts.push(String::new());
    total_tokens += 20;

    // Add pinned memories first (critical context for planning)
    if let Some(store) = stores.primary_store() {
        if let Ok(pinned_entries) = store.list_pinned() {
            if !pinned_entries.is_empty() {
                context_parts.push("## 📌 Critical Context (Pinned)".to_string());
                context_parts.push(String::new());
                for entry in &pinned_entries {
                    let title = entry.title.clone().unwrap_or_else(|| entry.preview(60));
                    context_parts.push(format!("### {} [{}]", entry.id, entry.entry_type));
                    if !title.is_empty() && title != entry.preview(60) {
                        context_parts.push(format!("**{title}**"));
                    }
                    context_parts.push(String::new());
                    context_parts.push(entry.content.clone());
                    context_parts.push(String::new());
                    total_tokens += estimate_tokens(&entry.content);
                }
                stats.pinned_included = pinned_entries.len();
            }
        }
    }

    // Task landscape with dependencies
    if let Some(ts) = stores.task_store {
        let mut all_tasks = Vec::new();

        if let Ok(in_progress) = ts.list(Some(TaskStatus::InProgress)) {
            all_tasks.extend(in_progress);
        }
        if let Ok(open) = ts.list(Some(TaskStatus::Open)) {
            all_tasks.extend(open);
        }
        if let Ok(blocked) = ts.list(Some(TaskStatus::Blocked)) {
            all_tasks.extend(blocked);
        }
        if plan_config.include_closed {
            if let Ok(closed) = ts.list(Some(TaskStatus::Closed)) {
                all_tasks.extend(closed.into_iter().take(5));
            }
            if let Ok(cancelled) = ts.list(Some(TaskStatus::Cancelled)) {
                all_tasks.extend(cancelled.into_iter().take(5));
            }
        }

        all_tasks.sort_by(|a, b| {
            let status_order = |s: &TaskStatus| match s {
                TaskStatus::InProgress => 0,
                TaskStatus::Blocked => 1,
                TaskStatus::Open => 2,
                TaskStatus::Closed => 3,
                TaskStatus::Cancelled => 3,
                TaskStatus::AwaitingMerge => 4,
            };
            status_order(&a.status)
                .cmp(&status_order(&b.status))
                .then_with(|| a.priority.cmp(&b.priority))
        });

        let mut seen_ids = HashSet::new();
        all_tasks.retain(|t| seen_ids.insert(t.id.clone()));

        if !all_tasks.is_empty() && budget_remaining(total_tokens) > 200 {
            context_parts.push("## 📋 Task Landscape".to_string());
            context_parts.push(String::new());

            let in_progress: Vec<_> = all_tasks
                .iter()
                .filter(|t| t.status == TaskStatus::InProgress)
                .collect();
            let blocked: Vec<_> = all_tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Blocked)
                .collect();
            let open: Vec<_> = all_tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Open)
                .collect();

            if !in_progress.is_empty() {
                context_parts.push("### 🔄 In Progress".to_string());
                for task in &in_progress {
                    if stats.tasks_included >= task_limit {
                        break;
                    }
                    let line = format_task_for_plan(task, ts);
                    total_tokens += estimate_tokens(&line);
                    context_parts.push(line);
                    stats.tasks_included += 1;
                }
                context_parts.push(String::new());
            }

            if !blocked.is_empty() && budget_remaining(total_tokens) > 100 {
                context_parts.push("### ⏸️ Blocked".to_string());
                for task in &blocked {
                    if stats.tasks_included >= task_limit {
                        break;
                    }
                    let blockers = ts.get_blockers(&task.id).unwrap_or_default();
                    let blocker_ids: Vec<_> = blockers.iter().map(|b| b.id.as_str()).collect();
                    let blocker_str = if blocker_ids.is_empty() {
                        String::new()
                    } else {
                        format!(" ← blocked by: {}", blocker_ids.join(", "))
                    };
                    let line = format!(
                        "- **{}** {} ({}){}\n",
                        task.id,
                        task.preview(50),
                        task.priority,
                        blocker_str
                    );
                    total_tokens += estimate_tokens(&line);
                    context_parts.push(line);
                    stats.tasks_included += 1;
                }
                context_parts.push(String::new());
            }

            if !open.is_empty() && budget_remaining(total_tokens) > 100 {
                context_parts.push("### ✅ Ready (Open)".to_string());
                for task in open
                    .iter()
                    .take(task_limit.saturating_sub(stats.tasks_included))
                {
                    let line = format!(
                        "- {} {} ({}) [{}]\n",
                        task.id,
                        task.preview(50),
                        task.priority,
                        task.task_type
                    );
                    total_tokens += estimate_tokens(&line);
                    context_parts.push(line);
                    stats.tasks_included += 1;
                }
                context_parts.push(String::new());
            }

            // Dependency tree
            if plan_config.show_dependencies && budget_remaining(total_tokens) > 200 {
                if let Ok(deps) = ts.list_dependencies(None) {
                    if !deps.is_empty() {
                        context_parts.push("### 🔗 Dependencies".to_string());
                        context_parts.push(String::new());
                        context_parts.push("```".to_string());
                        let tree = build_dependency_tree(&deps, &all_tasks);
                        let tree_tokens = estimate_tokens(&tree);
                        if tree_tokens < budget_remaining(total_tokens) - 50 {
                            context_parts.push(tree.clone());
                            total_tokens += tree_tokens;
                        } else {
                            let truncated: String =
                                tree.lines().take(15).collect::<Vec<_>>().join("\n");
                            context_parts.push(truncated);
                            context_parts.push(
                                "... (truncated, use `cas task dep list` for full view)"
                                    .to_string(),
                            );
                            total_tokens += 100;
                        }
                        context_parts.push("```".to_string());
                        context_parts.push(String::new());
                    }
                }
            }
        }
    }

    // Architecture and design rules
    if budget_remaining(total_tokens) > 150 {
        // Use cache if available, otherwise fall back to direct matching
        let matches_cwd = |rule: &Rule| -> bool {
            if let Some(cache) = stores.rule_match_cache {
                cache.matches(rule, &input.cwd)
            } else {
                rule_matches_path(rule, &input.cwd)
            }
        };

        let architecture_rules: Vec<_> = merged_rules
            .iter()
            .filter(|r| r.status == RuleStatus::Proven)
            .filter(|r| matches_cwd(r))
            .filter(|r| {
                r.tags.iter().any(|t| {
                    let t_lower = t.to_lowercase();
                    t_lower.contains("arch")
                        || t_lower.contains("design")
                        || t_lower.contains("pattern")
                        || t_lower.contains("convention")
                }) || r.tags.is_empty()
            })
            .collect();

        if !architecture_rules.is_empty() {
            context_parts.push("## 📏 Design Rules".to_string());
            context_parts.push(String::new());
            for rule in architecture_rules.iter().take(10) {
                let item = ContextItem::from_rule(rule);
                if item.tokens < 100 {
                    context_parts.push(format!("- **{}** {}", item.id, rule.content));
                } else {
                    context_parts.push(format!(
                        "- **{}** {} [{}] `cas show {}`",
                        item.id,
                        item.summary,
                        token_display(item.tokens),
                        item.id
                    ));
                }
                total_tokens += estimate_tokens(&item.summary) + 20;
                stats.rules_included += 1;
            }
            context_parts.push(String::new());
        }
    }

    // Planning hints
    context_parts.push("---".to_string());
    context_parts.push(String::new());
    context_parts.push("**Planning Tools:**".to_string());
    context_parts.push("- `mcp__cas__search` - Find related context".to_string());
    context_parts.push("- `mcp__cas__task` with action: list - Full task landscape".to_string());
    context_parts.push("- `mcp__cas__task` with action: dep_list - View dependencies".to_string());
    context_parts.push("- `mcp__cas__memory` with action: get - Get full details".to_string());
    context_parts.push(String::new());
    context_parts.push(format!(
        "**Context: {}** (plan mode budget: {})",
        token_display(total_tokens),
        token_display(token_budget)
    ));

    stats.total_tokens = total_tokens;
    Ok((
        remap_tool_prefix(&context_parts.join("\n"), tool_prefix),
        stats,
    ))
}

/// Format a task with dependency info for plan mode
fn format_task_for_plan(task: &Task, ts: &dyn TaskStore) -> String {
    let mut parts = vec![format!(
        "- **{}** {} ({}) [{}]",
        task.id,
        task.preview(50),
        task.priority,
        task.task_type
    )];

    if !task.description.is_empty() {
        let desc_preview = truncate(&task.description, 100);
        parts.push(format!("  > {desc_preview}"));
    }

    if let Ok(blockers) = ts.get_blockers(&task.id) {
        if !blockers.is_empty() {
            let blocker_ids: Vec<_> = blockers.iter().map(|b| b.id.as_str()).collect();
            parts.push(format!("  ⚠️ Blocked by: {}", blocker_ids.join(", ")));
        }
    }

    if let Ok(deps) = ts.get_dependents(&task.id) {
        let blocking: Vec<_> = deps
            .iter()
            .filter(|d| d.is_blocking())
            .map(|d| d.from_id.as_str())
            .collect();
        if !blocking.is_empty() {
            parts.push(format!("  → Blocks: {}", blocking.join(", ")));
        }
    }

    parts.join("\n")
}

/// Build a text representation of the dependency tree
fn build_dependency_tree(deps: &[Dependency], tasks: &[Task]) -> String {
    use std::collections::HashMap;

    let task_map: HashMap<&str, &Task> = tasks.iter().map(|t| (t.id.as_str(), t)).collect();

    let blocked_by: HashSet<&str> = deps
        .iter()
        .filter(|d| d.is_blocking())
        .map(|d| d.from_id.as_str())
        .collect();

    let blockers: HashSet<&str> = deps
        .iter()
        .filter(|d| d.is_blocking())
        .map(|d| d.to_id.as_str())
        .collect();

    let roots: Vec<&str> = blockers.difference(&blocked_by).copied().collect();

    let mut children: HashMap<&str, Vec<&str>> = HashMap::new();
    for dep in deps.iter().filter(|d| d.is_blocking()) {
        children
            .entry(dep.to_id.as_str())
            .or_default()
            .push(dep.from_id.as_str());
    }

    let mut output = Vec::new();
    let mut visited = HashSet::new();

    fn render_tree(
        task_id: &str,
        task_map: &HashMap<&str, &Task>,
        children: &HashMap<&str, Vec<&str>>,
        depth: usize,
        output: &mut Vec<String>,
        visited: &mut HashSet<String>,
    ) {
        if visited.contains(task_id) {
            return;
        }
        visited.insert(task_id.to_string());

        let indent = "  ".repeat(depth);
        let prefix = if depth == 0 { "📦" } else { "└─" };

        let task_info = task_map
            .get(task_id)
            .map(|t| format!("{} {} [{}]", t.preview(40), t.status, t.priority))
            .unwrap_or_else(|| "(unknown)".to_string());

        output.push(format!("{indent}{prefix} {task_id} {task_info}"));

        if let Some(child_ids) = children.get(task_id) {
            for child_id in child_ids {
                render_tree(child_id, task_map, children, depth + 1, output, visited);
            }
        }
    }

    for root in &roots {
        render_tree(root, &task_map, &children, 0, &mut output, &mut visited);
    }

    for task in tasks {
        if !visited.contains(&task.id)
            && (blocked_by.contains(task.id.as_str()) || blockers.contains(task.id.as_str()))
        {
            render_tree(&task.id, &task_map, &children, 0, &mut output, &mut visited);
        }
    }

    if output.is_empty() {
        "No blocking dependencies found.".to_string()
    } else {
        output.join("\n")
    }
}
