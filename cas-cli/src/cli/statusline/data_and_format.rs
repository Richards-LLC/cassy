use crate::cli::statusline::colors;
use crate::cli::statusline::{
    AgentContext, AgentCounts, HealthStatus, MemoryCounts, MyTaskInfo, OtherAgentWork, RuleCounts,
    SessionInfo, SkillCounts, StatusLineData, TaskCounts, UpdateInfo, WorktreeCounts,
};
use crate::config::Config;
use crate::store::{
    open_agent_store, open_rule_store, open_skill_store, open_store, open_task_store,
    open_worktree_store,
};
use crate::types::{AgentStatus, RuleStatus, SkillStatus, TaskStatus};
use crate::worktree::GitOperations;
use cas_core::Syncer;
use cas_types::DependencyType;
use serde::{Deserialize, Serialize};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const UPDATE_CHECK_CACHE_TTL_SECS: u64 = 24 * 60 * 60;
const UPDATE_CHECK_CACHE_FAILURE_TTL_SECS: u64 = 60 * 60;
const UPDATE_CHECK_TIMEOUT_MS: u64 = 1200;
const UPDATE_CHECK_URL: &str = "https://api.github.com/repos/Richards-LLC/cassy/releases/latest";
const UPDATE_CHECK_CACHE_RELATIVE: &str = "cache/update-check.json";

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct UpdateCheckCache {
    checked_at_unix: u64,
    latest_version: Option<String>,
    update_available: bool,
    failed: bool,
}

pub(crate) fn collect_status_data(
    session_info: Option<SessionInfo>,
    cas_root: std::path::PathBuf,
) -> anyhow::Result<StatusLineData> {
    let current_version = env!("CARGO_PKG_VERSION");
    // Open stores
    let store = open_store(&cas_root)?;
    let rule_store = open_rule_store(&cas_root)?;
    let task_store = open_task_store(&cas_root)?;
    let skill_store = open_skill_store(&cas_root)?;
    let agent_store = open_agent_store(&cas_root)?;
    let config = Config::load(&cas_root)?;

    // Agent counts
    let all_agents = agent_store.list(None).unwrap_or_default();
    let active_agents = all_agents
        .iter()
        .filter(|a| a.status == AgentStatus::Active)
        .count();
    let active_leases = agent_store.list_active_leases().unwrap_or_default();
    let claimed_task_count = active_leases.len();

    let agents = AgentCounts {
        active: active_agents,
        total: all_agents.len(),
        claimed_tasks: claimed_task_count,
    };

    // Task counts
    let ready_tasks = task_store.list_ready()?;
    let in_progress_tasks = task_store.list(Some(TaskStatus::InProgress))?;
    let blocked_tasks = task_store.list_blocked()?;
    let open_tasks = task_store.list(Some(TaskStatus::Open))?;

    // Available = ready tasks that are not claimed by any agent
    let claimed_task_ids: std::collections::HashSet<_> =
        active_leases.iter().map(|l| l.task_id.as_str()).collect();
    let available_count = ready_tasks
        .iter()
        .filter(|t| !claimed_task_ids.contains(t.id.as_str()))
        .count();

    // Orphaned = InProgress tasks that have no active lease (interrupted/abandoned work)
    let orphaned_count = in_progress_tasks
        .iter()
        .filter(|t| !claimed_task_ids.contains(t.id.as_str()))
        .count();

    let tasks = TaskCounts {
        ready: ready_tasks.len(),
        in_progress: in_progress_tasks.len(),
        blocked: blocked_tasks.len(),
        claimed: claimed_task_count,
        available: available_count,
        total_open: open_tasks.len() + in_progress_tasks.len(),
        orphaned: orphaned_count,
    };

    // Memory counts
    let entries = store.list()?;
    let pinned = store.list_pinned()?;
    let pending_extraction = store.list_pending(100)?;
    let helpful_count = entries.iter().filter(|e| e.feedback_score() > 0).count();

    let memories = MemoryCounts {
        total: entries.len(),
        pinned: pinned.len(),
        helpful: helpful_count,
        pending_extraction: pending_extraction.len(),
    };

    // Rule counts
    let rules = rule_store.list()?;
    let project_root = cas_root.parent().unwrap_or(std::path::Path::new("."));
    let syncer = Syncer::new(
        project_root.join(&config.sync.target),
        config.sync.min_helpful,
    );
    let proven_count = rules.iter().filter(|r| syncer.is_proven(r)).count();
    let stale_count = rules
        .iter()
        .filter(|r| r.status == RuleStatus::Stale)
        .count();

    let rule_counts = RuleCounts {
        total: rules.len(),
        proven: proven_count,
        stale: stale_count,
    };

    // Skill counts
    let all_skills = skill_store.list(None)?;
    let enabled_skills = skill_store.list(Some(SkillStatus::Enabled))?;

    let skills = SkillCounts {
        total: all_skills.len(),
        enabled: enabled_skills.len(),
    };

    // Worktree counts (only if feature is enabled)
    let worktrees = {
        let cwd = std::env::current_dir().unwrap_or_default();
        let git_context = GitOperations::get_context(&cwd).ok();

        // Skip worktree tracking if feature is disabled
        let worktrees_enabled = config
            .worktrees
            .as_ref()
            .map(|w| w.enabled)
            .unwrap_or(false);
        let (active_count, orphaned_count) = if worktrees_enabled {
            if let Ok(worktree_store) = open_worktree_store(&cas_root) {
                let active_worktrees = worktree_store.list_active().unwrap_or_default();

                // Count orphans (epic closed or agent dead)
                let orphaned = active_worktrees
                    .iter()
                    .filter(|wt| {
                        if !wt.path.exists() {
                            return true;
                        }
                        if let Some(ref epic_id) = wt.epic_id {
                            if let Ok(epic) = task_store.get(epic_id) {
                                if matches!(epic.status, TaskStatus::Closed) {
                                    return true;
                                }
                            }
                        }
                        if let Some(ref agent_id) = wt.created_by_agent {
                            if let Ok(agent) = agent_store.get(agent_id) {
                                if matches!(
                                    agent.status,
                                    AgentStatus::Stale | AgentStatus::Shutdown
                                ) {
                                    return true;
                                }
                            }
                        }
                        false
                    })
                    .count();

                (active_worktrees.len(), orphaned)
            } else {
                (0, 0)
            }
        } else {
            (0, 0)
        };

        WorktreeCounts {
            active: active_count,
            orphaned: orphaned_count,
            in_worktree: git_context.as_ref().map(|c| c.is_worktree).unwrap_or(false),
            current_branch: git_context.and_then(|c| c.branch),
        }
    };

    // Health status - check for active daemon via DB heartbeat
    let daemon_running = agent_store
        .is_daemon_active(60) // 60 second threshold
        .unwrap_or(false);
    let has_pending_work = !pending_extraction.is_empty();
    let has_blocked_tasks = !blocked_tasks.is_empty();

    let status = if has_blocked_tasks {
        "blocked".to_string()
    } else if has_pending_work && !daemon_running {
        "degraded".to_string()
    } else if has_pending_work {
        "pending".to_string()
    } else {
        "ok".to_string()
    };

    let health = HealthStatus {
        daemon_running,
        has_pending_work,
        has_blocked_tasks,
        status,
    };

    // Build agent context for multi-agent awareness
    let agent_context = {
        // Agent ID is the session_id (1:1 mapping)
        // The statusline runs as a separate process, so its PID differs from the MCP server's PID
        let my_agent_id: Option<String> = if let Some(ref info) = session_info {
            if let Some(ref sid) = info.session_id {
                // Session ID is the agent ID - verify agent exists
                agent_store.get(sid).ok().map(|a| a.id)
            } else {
                None
            }
        } else {
            // No session info - likely CLI usage without Claude Code
            None
        };

        // Get my claimed tasks with details
        let my_tasks: Vec<MyTaskInfo> = if let Some(ref agent_id) = my_agent_id {
            agent_store
                .list_agent_leases(agent_id)
                .unwrap_or_default()
                .into_iter()
                .filter_map(|lease| {
                    task_store.get(&lease.task_id).ok().map(|task| {
                        // Get blocked_by task IDs
                        let blocked_by = task_store
                            .get_dependencies(&task.id)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|dep| dep.dep_type == DependencyType::Blocks)
                            .map(|dep| dep.to_id)
                            .collect();

                        MyTaskInfo {
                            id: task.id,
                            title: task.title,
                            priority: task.priority.0,
                            blocked_by,
                        }
                    })
                })
                .collect()
        } else {
            Vec::new()
        };

        // Get other agents' work (excluding my own)
        let other_agents_working: Vec<OtherAgentWork> = active_leases
            .iter()
            .filter(|lease| my_agent_id.as_ref() != Some(&lease.agent_id))
            .filter_map(|lease| {
                let agent = all_agents.iter().find(|a| a.id == lease.agent_id)?;
                let task = task_store.get(&lease.task_id).ok()?;
                Some(OtherAgentWork {
                    agent_id: agent.id.clone(),
                    agent_name: agent.name.clone(),
                    task_title: task.title,
                })
            })
            .collect();

        AgentContext {
            my_agent_id,
            my_tasks,
            other_agents_working,
        }
    };

    Ok(StatusLineData {
        agents,
        tasks,
        memories,
        rules: rule_counts,
        skills,
        worktrees,
        health,
        update: get_update_info(&cas_root, current_version),
        session: session_info,
        agent_context,
    })
}

fn get_update_info(cas_root: &std::path::Path, current_version: &str) -> Option<UpdateInfo> {
    if std::env::var("CAS_DISABLE_UPDATE_CHECK")
        .ok()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
    {
        return None;
    }

    let now = now_unix();
    let cache_path = cas_root.join(UPDATE_CHECK_CACHE_RELATIVE);
    let mut cache = read_update_cache(&cache_path);

    let is_fresh = cache.as_ref().is_some_and(|c| {
        let ttl = if c.failed {
            UPDATE_CHECK_CACHE_FAILURE_TTL_SECS
        } else {
            UPDATE_CHECK_CACHE_TTL_SECS
        };
        now.saturating_sub(c.checked_at_unix) < ttl
    });

    if !is_fresh {
        cache = Some(fetch_and_store_update_cache(
            &cache_path,
            current_version,
            now,
            cache.as_ref(),
        ));
    }

    cache.and_then(|c| {
        if c.update_available {
            c.latest_version
                .filter(|v| is_newer_version(v, current_version))
                .map(|latest_version| UpdateInfo { latest_version })
        } else {
            None
        }
    })
}

fn read_update_cache(path: &std::path::Path) -> Option<UpdateCheckCache> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str::<UpdateCheckCache>(&raw).ok()
}

fn fetch_and_store_update_cache(
    cache_path: &std::path::Path,
    current_version: &str,
    checked_at_unix: u64,
    previous: Option<&UpdateCheckCache>,
) -> UpdateCheckCache {
    let fetched = fetch_latest_version(current_version).map(|latest| UpdateCheckCache {
        checked_at_unix,
        update_available: is_newer_version(&latest, current_version),
        latest_version: Some(latest),
        failed: false,
    });

    let cache = fetched.unwrap_or_else(|_| {
        let mut fallback = previous.cloned().unwrap_or_default();
        fallback.checked_at_unix = checked_at_unix;
        fallback.failed = true;
        fallback
    });

    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(serialized) = serde_json::to_string(&cache) {
        let _ = std::fs::write(cache_path, serialized);
    }

    cache
}

fn fetch_latest_version(current_version: &str) -> anyhow::Result<String> {
    #[derive(Debug, Deserialize)]
    struct ReleaseResponse {
        tag_name: String,
    }

    let response = ureq::AgentBuilder::new()
        .timeout(Duration::from_millis(UPDATE_CHECK_TIMEOUT_MS))
        .build()
        .get(UPDATE_CHECK_URL)
        .set("Accept", "application/vnd.github+json")
        .set("User-Agent", &format!("cas/{current_version}"))
        .call()?;

    let release: ReleaseResponse = response.into_json()?;
    Ok(release.tag_name.trim_start_matches('v').to_string())
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn is_newer_version(new: &str, current: &str) -> bool {
    let parse = |v: &str| -> Option<(u32, u32, u32)> {
        let parts: Vec<&str> = v.trim_start_matches('v').split('.').collect();
        if parts.len() >= 3 {
            Some((
                parts[0].parse().ok()?,
                parts[1].parse().ok()?,
                parts[2].split('-').next()?.parse().ok()?,
            ))
        } else {
            None
        }
    };

    match (parse(new), parse(current)) {
        (Some((n1, n2, n3)), Some((c1, c2, c3))) => (n1, n2, n3) > (c1, c2, c3),
        _ => false,
    }
}

pub(crate) fn format_status_line(data: &StatusLineData, no_color: bool, minimal: bool) -> String {
    let use_color = !no_color && std::env::var("NO_COLOR").is_err();

    if minimal {
        return format_minimal(data, use_color);
    }

    // Working mode: show task-focused view when agent has claimed tasks
    if !data.agent_context.my_tasks.is_empty() {
        return format_working_mode(data, use_color);
    }

    // Idle mode: show stats overview
    format_idle_mode(data, use_color)
}

/// Format the status line in working mode (task-focused view)
fn format_working_mode(data: &StatusLineData, use_color: bool) -> String {
    let sep = if use_color {
        format!(" {}│{} ", colors::DIM, colors::RESET)
    } else {
        " │ ".to_string()
    };

    let mut parts = Vec::new();

    // Show my primary task (first claimed task)
    if let Some(task) = data.agent_context.my_tasks.first() {
        // Truncate title to ~45 chars
        let title = if task.title.len() > 45 {
            format!("{}...", &task.title[..42])
        } else {
            task.title.clone()
        };

        let task_str = format!("{}: {}", task.id, title);
        let task_part = if use_color {
            format!("{}▸ {}{}", colors::YELLOW, task_str, colors::RESET)
        } else {
            format!("▸ {task_str}")
        };
        parts.push(task_part);

        // Show priority
        let priority_str = match task.priority {
            0 => "P0",
            1 => "P1",
            2 => "P2",
            3 => "P3",
            _ => "P4",
        };
        let priority_part = if use_color {
            let color = match task.priority {
                0 => colors::RED,
                1 => colors::YELLOW,
                _ => colors::DIM,
            };
            format!("{}{}{}", color, priority_str, colors::RESET)
        } else {
            priority_str.to_string()
        };
        parts.push(priority_part);

        // Show if blocked
        if !task.blocked_by.is_empty() {
            let blocked_str = if task.blocked_by.len() == 1 {
                format!("blocked by {}", task.blocked_by[0])
            } else {
                format!("blocked by {} tasks", task.blocked_by.len())
            };
            let blocked_part = if use_color {
                format!("{}⛔ {}{}", colors::RED, blocked_str, colors::RESET)
            } else {
                format!("⛔ {blocked_str}")
            };
            parts.push(blocked_part);
        }
    }

    // Show additional tasks if I have more than one
    if data.agent_context.my_tasks.len() > 1 {
        let extra = data.agent_context.my_tasks.len() - 1;
        let extra_str = format!("+{extra} more");
        let extra_part = if use_color {
            format!("{}{}{}", colors::DIM, extra_str, colors::RESET)
        } else {
            extra_str
        };
        parts.push(extra_part);
    }

    // Show other agents working
    if !data.agent_context.other_agents_working.is_empty() {
        let count = data.agent_context.other_agents_working.len();
        let agent_str = if count == 1 {
            "+1 agent".to_string()
        } else {
            format!("+{count} agents")
        };
        let agent_part = if use_color {
            format!("{}👥 {}{}", colors::CYAN, agent_str, colors::RESET)
        } else {
            format!("👥 {agent_str}")
        };
        parts.push(agent_part);
    }

    // Update availability
    if let Some(update) = &data.update {
        let update_str = format!("⬆ v{} available", update.latest_version);
        let update_part = if use_color {
            format!("{}{}{}", colors::YELLOW, update_str, colors::RESET)
        } else {
            update_str
        };
        parts.push(update_part);
    }

    // Format: "Cassy │ ▸ cas-ca75: Implement PreToolUse... │ P1 │ 👥 +2 agents"
    if use_color {
        format!(
            "{}Cassy{}{}{}",
            colors::BOLD,
            colors::RESET,
            sep,
            parts.join(&sep)
        )
    } else {
        format!("Cassy{}{}", sep, parts.join(&sep))
    }
}

/// Format the status line in idle mode (stats overview)
fn format_idle_mode(data: &StatusLineData, use_color: bool) -> String {
    let sep = if use_color {
        format!(" {}│{} ", colors::DIM, colors::RESET)
    } else {
        " │ ".to_string()
    };

    let mut parts = Vec::new();

    // Agents section (only show if multiple agents or any claims)
    if data.agents.active > 1 || data.agents.claimed_tasks > 0 {
        let agent_str = if data.agents.active == 1 {
            "1 agent".to_string()
        } else {
            format!("{} agents", data.agents.active)
        };
        let agent_part = if use_color {
            format!("{}👥 {}{}", colors::CYAN, agent_str, colors::RESET)
        } else {
            format!("👥 {agent_str}")
        };
        parts.push(agent_part);
    }

    // Task section - show available vs claimed when agents are active
    if data.agents.active > 1 || data.agents.claimed_tasks > 0 {
        // Multi-agent mode: show available (unclaimed ready tasks)
        let avail_str = if data.tasks.available == 1 {
            "1 available".to_string()
        } else {
            format!("{} available", data.tasks.available)
        };
        let avail_part = if use_color {
            format!("{}{}{}", colors::CYAN, avail_str, colors::RESET)
        } else {
            avail_str
        };
        parts.push(avail_part);

        // Show claimed tasks if any
        if data.tasks.claimed > 0 {
            let claimed_str = if data.tasks.claimed == 1 {
                "1 claimed".to_string()
            } else {
                format!("{} claimed", data.tasks.claimed)
            };
            let claimed_part = if use_color {
                format!("{}🔒 {}{}", colors::YELLOW, claimed_str, colors::RESET)
            } else {
                format!("🔒 {claimed_str}")
            };
            parts.push(claimed_part);
        }
    } else {
        // Single-agent mode: just show ready tasks
        let ready_str = if data.tasks.ready == 1 {
            "1 ready task".to_string()
        } else {
            format!("{} ready tasks", data.tasks.ready)
        };
        let ready_part = if use_color {
            format!("{}{}{}", colors::CYAN, ready_str, colors::RESET)
        } else {
            ready_str
        };
        parts.push(ready_part);
    }

    // In progress section (yellow - actively claimed work only)
    let actively_working = data.tasks.in_progress.saturating_sub(data.tasks.orphaned);
    if actively_working > 0 {
        let wip_str = if actively_working == 1 {
            "1 in progress".to_string()
        } else {
            format!("{actively_working} in progress")
        };
        let wip_part = if use_color {
            format!("{}▸ {}{}", colors::YELLOW, wip_str, colors::RESET)
        } else {
            format!("▸ {wip_str}")
        };
        parts.push(wip_part);
    }

    // Orphaned section (red/orange - interrupted work needing attention)
    if data.tasks.orphaned > 0 {
        let orphan_str = if data.tasks.orphaned == 1 {
            "1 orphaned".to_string()
        } else {
            format!("{} orphaned", data.tasks.orphaned)
        };
        let orphan_part = if use_color {
            format!("{}⚡ {}{}", colors::RED, orphan_str, colors::RESET)
        } else {
            format!("⚡ {orphan_str}")
        };
        parts.push(orphan_part);
    }

    // Blocked tasks section (red/orange - needs attention)
    if data.tasks.blocked > 0 {
        let blocked_str = if data.tasks.blocked == 1 {
            "1 blocked".to_string()
        } else {
            format!("{} blocked", data.tasks.blocked)
        };
        let blocked_part = if use_color {
            format!("{}⚠ {}{}", colors::RED, blocked_str, colors::RESET)
        } else {
            format!("⚠ {blocked_str}")
        };
        parts.push(blocked_part);
    }

    // Worktree section (show if in worktree or have orphans)
    if data.worktrees.in_worktree {
        let branch = data
            .worktrees
            .current_branch
            .as_deref()
            .unwrap_or("unknown");
        let wt_part = if use_color {
            format!("{}🌿 {}{}", colors::CYAN, branch, colors::RESET)
        } else {
            format!("🌿 {branch}")
        };
        parts.push(wt_part);
    } else if data.worktrees.active > 0 {
        let wt_str = if data.worktrees.active == 1 {
            "1 worktree".to_string()
        } else {
            format!("{} worktrees", data.worktrees.active)
        };
        let wt_part = if use_color {
            format!("{}🌿 {}{}", colors::DIM, wt_str, colors::RESET)
        } else {
            format!("🌿 {wt_str}")
        };
        parts.push(wt_part);
    }

    // Orphaned worktrees warning
    if data.worktrees.orphaned > 0 {
        let orphan_wt_str = if data.worktrees.orphaned == 1 {
            "1 orphan wt".to_string()
        } else {
            format!("{} orphan wts", data.worktrees.orphaned)
        };
        let orphan_wt_part = if use_color {
            format!("{}🗑 {}{}", colors::YELLOW, orphan_wt_str, colors::RESET)
        } else {
            format!("🗑 {orphan_wt_str}")
        };
        parts.push(orphan_wt_part);
    }

    // Memories section (default color)
    let mem_str = if data.memories.total == 1 {
        "1 memory".to_string()
    } else {
        format!("{} memories", data.memories.total)
    };
    parts.push(mem_str);

    // Rules section (default color)
    let rules_str = if data.rules.proven == 1 {
        "1 rule".to_string()
    } else {
        format!("{} rules", data.rules.proven)
    };
    parts.push(rules_str);

    // Update availability
    if let Some(update) = &data.update {
        let update_str = format!("⬆ v{} available", update.latest_version);
        let update_part = if use_color {
            format!("{}{}{}", colors::YELLOW, update_str, colors::RESET)
        } else {
            update_str
        };
        parts.push(update_part);
    }

    // Health indicator
    let health_indicator = match data.health.status.as_str() {
        "ok" => {
            if use_color {
                format!("{}✓ healthy{}", colors::GREEN, colors::RESET)
            } else {
                "✓ healthy".to_string()
            }
        }
        "pending" => {
            let pending = data.memories.pending_extraction;
            let pending_str = if pending == 1 {
                "1 pending".to_string()
            } else {
                format!("{pending} pending")
            };
            if use_color {
                format!("{}⏳ {}{}", colors::YELLOW, pending_str, colors::RESET)
            } else {
                format!("⏳ {pending_str}")
            }
        }
        "blocked" => {
            // Note: blocked tasks are now shown separately, health just confirms healthy
            if use_color {
                format!("{}✓ healthy{}", colors::GREEN, colors::RESET)
            } else {
                "✓ healthy".to_string()
            }
        }
        "degraded" => {
            if use_color {
                format!("{}✗ daemon offline{}", colors::RED, colors::RESET)
            } else {
                "✗ daemon offline".to_string()
            }
        }
        _ => "? unknown".to_string(),
    };
    parts.push(health_indicator);

    // Format: "Cassy │ 128 ready tasks │ 28 blocked │ 993 memories │ 3 rules │ ✓ healthy"
    if use_color {
        format!(
            "{}Cassy{}{}{}",
            colors::BOLD,
            colors::RESET,
            sep,
            parts.join(&sep)
        )
    } else {
        format!("Cassy{}{}", sep, parts.join(&sep))
    }
}

fn format_minimal(data: &StatusLineData, use_color: bool) -> String {
    // Compact but readable: "Cassy │ 4a │ 128r 2w 28b │ 993m │ 3ru │ ✓"
    let mut parts = Vec::new();

    // Agents (only if multi-agent)
    if data.agents.active > 1 {
        parts.push(format!("{}a", data.agents.active));
    }

    // Tasks
    let mut task_parts = vec![];
    let actively_working = data.tasks.in_progress.saturating_sub(data.tasks.orphaned);
    if data.agents.active > 1 || data.agents.claimed_tasks > 0 {
        task_parts.push(format!("{}av", data.tasks.available));
        if data.tasks.claimed > 0 {
            task_parts.push(format!("{}cl", data.tasks.claimed));
        }
    } else {
        task_parts.push(format!("{}r", data.tasks.ready));
    }
    if actively_working > 0 {
        task_parts.push(format!("{actively_working}w"));
    }
    if data.tasks.orphaned > 0 {
        task_parts.push(format!("{}o", data.tasks.orphaned));
    }
    if data.tasks.blocked > 0 {
        task_parts.push(format!("{}b", data.tasks.blocked));
    }

    let health = match data.health.status.as_str() {
        "ok" | "blocked" => "✓",
        "pending" => "⏳",
        "degraded" => "✗",
        _ => "?",
    };

    let update_str = data
        .update
        .as_ref()
        .map(|u| format!(" ↑{}", u.latest_version))
        .unwrap_or_default();

    if use_color {
        let agent_str = if data.agents.active > 1 {
            format!("{}{}a{} ", colors::CYAN, data.agents.active, colors::RESET)
        } else {
            String::new()
        };

        let task_str = {
            let mut s = if data.agents.active > 1 || data.agents.claimed_tasks > 0 {
                let mut t = format!(
                    "{}{}av{}",
                    colors::CYAN,
                    data.tasks.available,
                    colors::RESET
                );
                if data.tasks.claimed > 0 {
                    t.push_str(&format!(
                        " {}{}cl{}",
                        colors::YELLOW,
                        data.tasks.claimed,
                        colors::RESET
                    ));
                }
                t
            } else {
                format!("{}{}r{}", colors::CYAN, data.tasks.ready, colors::RESET)
            };
            if actively_working > 0 {
                s.push_str(&format!(
                    " {}{}w{}",
                    colors::YELLOW,
                    actively_working,
                    colors::RESET
                ));
            }
            if data.tasks.orphaned > 0 {
                s.push_str(&format!(
                    " {}{}o{}",
                    colors::RED,
                    data.tasks.orphaned,
                    colors::RESET
                ));
            }
            if data.tasks.blocked > 0 {
                s.push_str(&format!(
                    " {}{}b{}",
                    colors::RED,
                    data.tasks.blocked,
                    colors::RESET
                ));
            }
            s
        };

        let health_colored = match data.health.status.as_str() {
            "ok" | "blocked" => format!("{}✓{}", colors::GREEN, colors::RESET),
            "pending" => format!("{}⏳{}", colors::YELLOW, colors::RESET),
            "degraded" => format!("{}✗{}", colors::RED, colors::RESET),
            _ => "?".to_string(),
        };

        let update_colored = data
            .update
            .as_ref()
            .map(|u| format!(" {}↑{}{}", colors::YELLOW, u.latest_version, colors::RESET))
            .unwrap_or_default();

        format!(
            "{}Cassy{} {}│{} {}{} {}│{} {}m {}│{} {}ru {}│{} {}{}",
            colors::BOLD,
            colors::RESET,
            colors::DIM,
            colors::RESET,
            agent_str,
            task_str,
            colors::DIM,
            colors::RESET,
            data.memories.total,
            colors::DIM,
            colors::RESET,
            data.rules.proven,
            colors::DIM,
            colors::RESET,
            health_colored,
            update_colored
        )
    } else {
        let agent_prefix = if data.agents.active > 1 {
            format!("{}a ", data.agents.active)
        } else {
            String::new()
        };

        format!(
            "Cassy │ {}{} │ {}m │ {}ru │ {}{}",
            agent_prefix,
            task_parts.join(" "),
            data.memories.total,
            data.rules.proven,
            health,
            update_str
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_data() -> StatusLineData {
        StatusLineData {
            agents: AgentCounts {
                active: 1,
                total: 1,
                claimed_tasks: 0,
            },
            tasks: TaskCounts {
                ready: 2,
                in_progress: 0,
                blocked: 0,
                claimed: 0,
                available: 2,
                total_open: 2,
                orphaned: 0,
            },
            memories: MemoryCounts {
                total: 10,
                pinned: 0,
                helpful: 0,
                pending_extraction: 0,
            },
            rules: RuleCounts {
                total: 2,
                proven: 2,
                stale: 0,
            },
            skills: SkillCounts {
                total: 3,
                enabled: 2,
            },
            worktrees: WorktreeCounts {
                active: 0,
                orphaned: 0,
                in_worktree: false,
                current_branch: None,
            },
            health: HealthStatus {
                daemon_running: true,
                has_pending_work: false,
                has_blocked_tasks: false,
                status: "ok".to_string(),
            },
            update: None,
            session: None,
            agent_context: AgentContext {
                my_agent_id: None,
                my_tasks: vec![],
                other_agents_working: vec![],
            },
        }
    }

    #[test]
    fn newer_version_detection_handles_semver() {
        assert!(is_newer_version("0.5.5", "0.5.4"));
        assert!(is_newer_version("v1.0.0", "0.9.9"));
        assert!(!is_newer_version("0.5.4", "0.5.4"));
        assert!(!is_newer_version("0.5.3", "0.5.4"));
    }

    #[test]
    fn idle_mode_shows_update_segment() {
        let mut data = sample_data();
        data.update = Some(UpdateInfo {
            latest_version: "0.6.0".to_string(),
        });

        let line = format_idle_mode(&data, false);
        assert!(line.contains("⬆ v0.6.0 available"));
    }

    #[test]
    fn minimal_mode_shows_update_segment() {
        let mut data = sample_data();
        data.update = Some(UpdateInfo {
            latest_version: "0.6.0".to_string(),
        });

        let line = format_minimal(&data, false);
        assert!(line.contains("↑0.6.0"));
    }
}
