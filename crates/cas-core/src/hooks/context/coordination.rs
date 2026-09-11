use crate::hooks::config::HooksConfig;
use crate::hooks::context::{estimate_tokens, truncate};
use cas_store::{AgentStore, TaskStore};
use cas_types::{Agent, AgentRole, AgentStatus};

pub(crate) fn render_factory_coordination(
    context_parts: &mut Vec<String>,
    total_tokens: &mut usize,
    current_agent: Option<&Agent>,
    other_agents: &[&Agent],
    agent_store: &dyn AgentStore,
    task_store: Option<&dyn TaskStore>,
    config: &dyn HooksConfig,
) {
    // Find supervisor and workers among all agents
    let all_agents: Vec<&Agent> = std::iter::once(current_agent)
        .flatten()
        .chain(other_agents.iter().copied())
        .collect();

    let supervisor = all_agents
        .iter()
        .find(|a| a.role == AgentRole::Supervisor)
        .copied();
    // Scope workers to CAS_FACTORY_WORKER_NAMES when set (prevents showing
    // stale workers from other sessions that share the same database).
    let owned_workers = supervisor_owned_workers();
    let workers: Vec<&Agent> = all_agents
        .iter()
        .filter(|a| {
            a.role == AgentRole::Worker
                && owned_workers
                    .as_ref()
                    .is_none_or(|set| set.contains(&a.name))
        })
        .copied()
        .collect();

    // Determine current agent identity from either database or env vars
    // This handles cases where hooks run in different processes than registration
    let (self_role, self_name, self_id): (Option<AgentRole>, Option<String>, Option<String>) =
        if let Some(agent) = current_agent {
            (
                Some(agent.role),
                Some(agent.name.clone()),
                Some(agent.id.clone()),
            )
        } else {
            // Fallback to env vars when PID-based lookup fails
            let role = std::env::var("CAS_AGENT_ROLE").ok().and_then(|r| {
                match r.to_lowercase().as_str() {
                    "supervisor" => Some(AgentRole::Supervisor),
                    "worker" => Some(AgentRole::Worker),
                    _ => None,
                }
            });
            let name = std::env::var("CAS_AGENT_NAME").ok();
            let id = std::env::var("CAS_AGENT_ID").ok();
            (role, name, id)
        };

    // Show current agent identity
    if let (Some(role), Some(name)) = (self_role, self_name.as_ref()) {
        let role_str = match role {
            AgentRole::Worker => "worker",
            AgentRole::Supervisor => "supervisor",
            AgentRole::Director => "director",
            AgentRole::Standard => "agent",
        };

        let task_info = if let Some(agent) = current_agent {
            let leases = agent_store.list_agent_leases(&agent.id).unwrap_or_default();
            if !leases.is_empty() {
                let task_ids: Vec<_> = leases.iter().take(2).map(|l| l.task_id.as_str()).collect();
                task_ids.join(", ")
            } else {
                "idle".to_string()
            }
        } else {
            "via env".to_string()
        };

        let line = format!("**You**: {name} ({role_str}) — {task_info}");
        *total_tokens += estimate_tokens(&line);
        context_parts.push(line);

        // Inject role-specific guidance
        inject_role_guidance(context_parts, total_tokens, role, config);
    }

    // Determine if we should show supervisor info.
    // Only factory participants (worker/supervisor) should see a direct supervisor line.
    // Standalone/standard agents can coexist in the same project DB and should not be
    // told they have "my supervisor".
    let is_self_supervisor = self_role == Some(AgentRole::Supervisor);
    let is_factory_participant = is_factory_participant(self_role);
    let should_show_supervisor = supervisor.is_some_and(|sup| {
        if !is_factory_participant {
            false
        } else if !is_self_supervisor {
            // We're not a supervisor, always show supervisor info
            true
        } else {
            // We are a supervisor - only show if it's a different supervisor
            // Compare by ID if available, otherwise by name
            match (&self_id, &self_name) {
                (Some(our_id), _) => sup.id != *our_id,
                (None, Some(our_name)) => sup.name != *our_name,
                (None, None) => false, // Can't determine, don't show to avoid duplication
            }
        }
    });

    if let Some(sup) = supervisor {
        if should_show_supervisor {
            let leases = agent_store.list_agent_leases(&sup.id).unwrap_or_default();

            // Try to find active EPIC
            let epic_info = if let Some(ts) = task_store {
                leases
                    .iter()
                    .filter_map(|l| ts.get(&l.task_id).ok())
                    .find(|t| t.task_type == cas_types::TaskType::Epic)
                    .map(|t| format!("EPIC {}", t.id))
                    .unwrap_or_else(|| format!("{} task(s)", leases.len()))
            } else {
                format!("{} task(s)", leases.len())
            };

            let line = format!(
                "**Supervisor**: {} — {} ({} workers)",
                sup.name,
                epic_info,
                workers.len()
            );
            *total_tokens += estimate_tokens(&line);
            context_parts.push(line);
        }
    }

    // Show workers list (supervisors see detailed list, workers see compact list)
    if !workers.is_empty() {
        if is_self_supervisor {
            // Supervisor sees detailed worker list with status
            context_parts.push(String::new());
            let header = "### Workers".to_string();
            *total_tokens += estimate_tokens(&header);
            context_parts.push(header);

            for worker in &workers {
                let leases = agent_store
                    .list_agent_leases(&worker.id)
                    .unwrap_or_default();
                let task_info = if !leases.is_empty() {
                    leases[0].task_id.clone()
                } else {
                    "idle".to_string()
                };
                let status_emoji = match worker.status {
                    AgentStatus::Active => "🟢",
                    AgentStatus::Idle => "🟡",
                    AgentStatus::Stale => "🟠",
                    _ => "⚫",
                };
                let line = format!("- {} {} — {}", status_emoji, worker.name, task_info);
                *total_tokens += estimate_tokens(&line);
                context_parts.push(line);
            }
        } else if self_role == Some(AgentRole::Worker) {
            // Workers see compact list of other workers (excluding self)
            let other_workers: Vec<_> = workers
                .iter()
                .filter(|w| {
                    // Exclude self by ID or name
                    match (&self_id, &self_name) {
                        (Some(our_id), _) => w.id != *our_id,
                        (None, Some(our_name)) => w.name != *our_name,
                        (None, None) => true,
                    }
                })
                .take(5)
                .collect();

            if !other_workers.is_empty() {
                let worker_names: Vec<_> = other_workers.iter().map(|w| w.name.as_str()).collect();
                let line = format!("**Other workers**: {}", worker_names.join(", "));
                *total_tokens += estimate_tokens(&line);
                context_parts.push(line);
            }
        }
    }
}

pub(crate) fn is_factory_participant(self_role: Option<AgentRole>) -> bool {
    matches!(self_role, Some(AgentRole::Worker | AgentRole::Supervisor))
}

/// Returns the set of worker names this supervisor owns, derived from `CAS_FACTORY_WORKER_NAMES`.
/// Returns `None` when not running as a supervisor or when the variable is absent.
fn supervisor_owned_workers() -> Option<std::collections::HashSet<String>> {
    let role = std::env::var("CAS_AGENT_ROLE").unwrap_or_default();
    if !role.eq_ignore_ascii_case("supervisor") {
        return None;
    }
    let csv = std::env::var("CAS_FACTORY_WORKER_NAMES").ok()?;
    if csv.trim().is_empty() {
        return None;
    }
    Some(
        csv.split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect(),
    )
}

/// Inject role-specific guidance into context
fn inject_role_guidance(
    context_parts: &mut Vec<String>,
    total_tokens: &mut usize,
    role: AgentRole,
    config: &dyn HooksConfig,
) {
    let mut guidance = match role {
        AgentRole::Supervisor => config.supervisor_guidance(),
        AgentRole::Worker => config.worker_guidance(),
        _ => return,
    };

    // SessionStart context hint for supervisors when workers are Codex.
    //
    // EPIC cas-8888 (cas-fd9f): this note describes what the SUPERVISOR
    // itself (not the Codex workers it's coordinating) may call — so it
    // needs the supervisor's OWN tool prefix, which depends on the
    // supervisor's OWN harness, not the workers' Codex harness. Previously
    // hardcoded `mcp__cs__verification` unconditionally, which was already
    // wrong for a Claude (or Grok) supervisor coordinating Codex workers —
    // neither can call a `mcp__cs__`-namespaced tool from their own process.
    // Authored here against the `mcp__cas__` baseline like every other
    // contributor to this context buffer; `build_context_with_stores`
    // remaps the whole assembled text to the reader's real prefix exactly
    // once, at the end (see `remap_tool_prefix`'s doc comment).
    if role == AgentRole::Supervisor
        && std::env::var("CAS_FACTORY_WORKER_CLI")
            .map(|v| v.eq_ignore_ascii_case("codex"))
            .unwrap_or(false)
    {
        guidance.push_str(
            "\n\n## Codex Worker Coordination Note\n\
Workers are running Codex. Be explicit in assignments: include task id, acceptance criteria, required checks, and update cadence. Successful task action=start is authoritative assignment acceptance; no prose ACK is required. Ordinary worker updates surface through the inbox on the next turn; only authenticated typed blocker, merge, verification, or lifecycle events may wake an idle supervisor. Read task state and execution evidence before sending corrective prompts. For task closure, Codex workers must attempt task action=close first. Follow the returned verification-required or MERGE REQUIRED workflow: handle verification with task-verifier or mcp__cas__verification, or merge the delivery as requested. After the merge, the worker re-closes the task.",
        );
    }
    *total_tokens += estimate_tokens(&guidance);
    context_parts.push(guidance);
}

/// Render normal mode agent coordination context
///
/// Shows full agent details with PIDs and task hierarchies.
pub(crate) fn render_normal_coordination(
    context_parts: &mut Vec<String>,
    total_tokens: &mut usize,
    current_agent: Option<&Agent>,
    other_agents: &[&Agent],
    agent_store: &dyn AgentStore,
    task_store: Option<&dyn TaskStore>,
) {
    // Show current agent info
    if let Some(agent) = current_agent {
        let leases = agent_store.list_agent_leases(&agent.id).unwrap_or_default();
        let line = format!(
            "**You**: {} ({}) — {} task(s) claimed",
            agent.name,
            agent.id,
            leases.len()
        );
        *total_tokens += estimate_tokens(&line);
        context_parts.push(line);

        // Show claimed task IDs if any
        if !leases.is_empty() {
            let task_ids: Vec<_> = leases.iter().map(|l| l.task_id.as_str()).collect();
            let tasks_line = format!("  Claimed: {}", task_ids.join(", "));
            *total_tokens += estimate_tokens(&tasks_line);
            context_parts.push(tasks_line);
        }
    }

    // Show other active agents with their tasks
    if !other_agents.is_empty() {
        context_parts.push(String::new());
        context_parts.push("**Other Agents**:".to_string());
        for agent in other_agents.iter().take(5) {
            let leases = agent_store.list_agent_leases(&agent.id).unwrap_or_default();
            let status_indicator = match agent.status {
                AgentStatus::Active => "●",
                AgentStatus::Idle => "○",
                _ => "◌",
            };
            let line = format!(
                "- {} {} (PID {}) ({}) — {} task(s)",
                status_indicator,
                agent.name,
                agent
                    .pid
                    .map(|p| p.to_string())
                    .unwrap_or_else(|| "?".to_string()),
                agent.id,
                leases.len()
            );
            *total_tokens += estimate_tokens(&line);
            context_parts.push(line);

            // Show what tasks this agent is working on
            if !leases.is_empty() {
                // Get task details if task store is available
                if let Some(ts) = task_store {
                    for lease in leases.iter().take(3) {
                        if let Ok(task) = ts.get(&lease.task_id) {
                            let task_line =
                                format!("    └─ {} {}", task.id, truncate(&task.title, 40));
                            *total_tokens += estimate_tokens(&task_line);
                            context_parts.push(task_line);
                        }
                    }
                    if leases.len() > 3 {
                        let more_tasks = format!("    └─ ...and {} more", leases.len() - 3);
                        *total_tokens += estimate_tokens(&more_tasks);
                        context_parts.push(more_tasks);
                    }
                } else {
                    // Fallback: just show task IDs
                    let task_ids: Vec<_> = leases.iter().map(|l| l.task_id.as_str()).collect();
                    let tasks_line = format!("    └─ {}", task_ids.join(", "));
                    *total_tokens += estimate_tokens(&tasks_line);
                    context_parts.push(tasks_line);
                }
            }
        }
        if other_agents.len() > 5 {
            let more = format!("  ...and {} more agents", other_agents.len() - 5);
            *total_tokens += estimate_tokens(&more);
            context_parts.push(more);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hooks::config::DefaultHooksConfig;

    #[test]
    fn codex_worker_coordination_entry_preserves_worker_close() {
        const CHILD: &str = "CAS_TEST_COORDINATION_CLOSE_ENTRY";
        // Exercise the real environment-gated entry without mutating the test
        // runner's environment or adding a production-only-for-tests seam.
        if std::env::var_os(CHILD).is_none() {
            for worker_cli in ["codex", "CoDeX", "claude"] {
                let output = std::process::Command::new(std::env::current_exe().unwrap())
                    .args([
                        "--exact",
                        "hooks::context::coordination::tests::codex_worker_coordination_entry_preserves_worker_close",
                        "--nocapture",
                    ])
                    .env(CHILD, "1")
                    .env("CAS_FACTORY_WORKER_CLI", worker_cli)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "worker_cli={worker_cli}: {}\n{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                );
                assert!(String::from_utf8_lossy(&output.stdout).contains("1 passed"));
            }
            return;
        }

        let config = DefaultHooksConfig::default();
        for role in [AgentRole::Supervisor, AgentRole::Worker] {
            let mut parts = Vec::new();
            let mut tokens = 0;
            inject_role_guidance(&mut parts, &mut tokens, role, &config);
            let emitted = parts.join("\n");
            let expects_note = role == AgentRole::Supervisor
                && std::env::var("CAS_FACTORY_WORKER_CLI")
                    .unwrap()
                    .eq_ignore_ascii_case("codex");
            assert_eq!(
                emitted.contains("Codex Worker Coordination Note"),
                expects_note
            );
            assert!(!emitted.contains("ask you to verify and close on their behalf"));
            if expects_note {
                for required in [
                    "authoritative assignment acceptance",
                    "no prose ACK is required",
                    "inbox on the next turn",
                    "only authenticated typed",
                    "workers must attempt task action=close first",
                    "returned verification-required or MERGE REQUIRED workflow",
                    "mcp__cas__verification",
                    "After the merge, the worker re-closes the task",
                ] {
                    assert!(emitted.contains(required), "missing {required}: {emitted}");
                }
            }
            assert!(tokens > 0);
        }
    }
}
