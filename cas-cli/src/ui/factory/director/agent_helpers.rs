//! Shared helpers for agent status display across factory panels.
//!
//! Consolidates heartbeat detection, status icon/color resolution,
//! agent counting, and task lookup logic used by factory_radar,
//! mission_workers, and mission_epic.

use cas_types::{AgentStatus, TaskStatus};
use chrono::Utc;
use ratatui::prelude::Color;

use super::data::DirectorData;
use crate::ui::theme::{Icons, MinionsIcons, Palette};

/// Agents with no heartbeat for this many seconds are considered disconnected.
pub const HEARTBEAT_TIMEOUT_SECS: i64 = 300;

/// Returns the number of seconds since the agent's last heartbeat,
/// or `i64::MAX` if no heartbeat has been recorded.
pub fn heartbeat_age_secs(agent: &cas_factory::AgentSummary) -> i64 {
    agent
        .last_heartbeat
        .map(|hb| Utc::now().signed_duration_since(hb).num_seconds())
        .unwrap_or(i64::MAX)
}

/// Whether the agent should be treated as disconnected.
pub fn is_disconnected(agent: &cas_factory::AgentSummary) -> bool {
    heartbeat_age_secs(agent) > HEARTBEAT_TIMEOUT_SECS
}

/// Resolve the status icon string and color for an agent.
///
/// Takes heartbeat into account: disconnected agents always show the dead icon
/// regardless of their reported status.
pub fn agent_status_icon(
    agent: &cas_factory::AgentSummary,
    palette: &Palette,
    minions: bool,
) -> (&'static str, Color) {
    if is_disconnected(agent) {
        let icon = if minions {
            MinionsIcons::AGENT_DEAD
        } else {
            "\u{2298}"
        };
        (icon, palette.agent_dead)
    } else {
        match agent.status {
            AgentStatus::Active => {
                let icon = if minions {
                    MinionsIcons::AGENT_ACTIVE
                } else {
                    Icons::CIRCLE_FILLED
                };
                (icon, palette.agent_active)
            }
            AgentStatus::Idle => {
                let icon = if minions {
                    MinionsIcons::AGENT_IDLE
                } else {
                    Icons::CIRCLE_HALF
                };
                (icon, palette.agent_idle)
            }
            _ => {
                let icon = if minions {
                    MinionsIcons::AGENT_DEAD
                } else {
                    Icons::CIRCLE_EMPTY
                };
                (icon, palette.agent_dead)
            }
        }
    }
}

/// Resolve agent status icon using simple Unicode characters (for compact views).
pub fn agent_status_icon_simple(
    agent: &cas_factory::AgentSummary,
    palette: &Palette,
    minions: bool,
) -> (&'static str, Color) {
    if minions {
        match agent.status {
            AgentStatus::Active => (MinionsIcons::AGENT_ACTIVE, palette.agent_active),
            AgentStatus::Idle => (MinionsIcons::AGENT_IDLE, palette.agent_idle),
            _ => (MinionsIcons::AGENT_DEAD, palette.agent_dead),
        }
    } else {
        match agent.status {
            AgentStatus::Active => ("\u{25cf}", palette.agent_active), // ●
            AgentStatus::Idle => ("\u{25cb}", palette.agent_idle),     // ○
            _ => ("\u{2298}", palette.agent_dead),                     // ⊘
        }
    }
}

/// Heartbeat-aware agent status counts.
#[derive(Debug, Default)]
pub struct AgentStatusCounts {
    pub active: usize,
    pub idle: usize,
    pub dead: usize,
}

/// Count agents by effective status, treating disconnected agents as dead.
pub fn count_agent_statuses(agents: &[cas_factory::AgentSummary]) -> AgentStatusCounts {
    let mut counts = AgentStatusCounts::default();
    for agent in agents {
        if is_disconnected(agent) {
            counts.dead += 1;
        } else {
            match agent.status {
                AgentStatus::Active => counts.active += 1,
                AgentStatus::Idle => counts.idle += 1,
                _ => counts.dead += 1,
            }
        }
    }
    counts
}

/// Find the task currently assigned to an agent.
///
/// Searches in_progress_tasks first, then ready_tasks.
pub fn find_agent_task<'a>(
    agent: &cas_factory::AgentSummary,
    data: &'a DirectorData,
) -> Option<&'a cas_factory::TaskSummary> {
    let task_id = agent.current_task.as_ref()?;
    data.in_progress_tasks
        .iter()
        .chain(data.ready_tasks.iter())
        .find(|t| &t.id == task_id)
}

/// Find the current in-progress task for an agent by explicit current_task,
/// agent ID, or display name.
pub fn find_agent_in_progress_task<'a>(
    agent: &cas_factory::AgentSummary,
    data: &'a DirectorData,
) -> Option<&'a cas_factory::TaskSummary> {
    if let Some(task_id) = agent.current_task.as_ref() {
        if let Some(task) = data.in_progress_tasks.iter().find(|t| &t.id == task_id) {
            return Some(task);
        }
    }

    data.in_progress_tasks.iter().find(|task| {
        task.assignee
            .as_deref()
            .is_some_and(|assignee| assignee == agent.id || assignee == agent.name)
    })
}

/// Map a task status to a display icon.
pub fn task_status_icon(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::InProgress => Icons::SPINNER_STATIC,
        TaskStatus::Blocked => Icons::BLOCKED,
        TaskStatus::AwaitingMerge => Icons::CLOCK,
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use cas_types::{Priority, TaskType};

    use super::*;

    fn agent(id: &str, name: &str, current_task: Option<&str>) -> cas_factory::AgentSummary {
        cas_factory::AgentSummary {
            id: id.to_string(),
            name: name.to_string(),
            status: AgentStatus::Active,
            registered_at: Utc::now(),
            current_task: current_task.map(str::to_string),
            latest_activity: None,
            last_heartbeat: Some(Utc::now()),
            pending_messages: 0,
            pending_supervisor_messages: 0,
            latest_supervisor_message_at: None,
            active_lease: None,
            effort: None,
        }
    }

    fn task(id: &str, assignee: Option<&str>) -> cas_factory::TaskSummary {
        cas_factory::TaskSummary {
            id: id.to_string(),
            title: format!("Task {id}"),
            status: TaskStatus::InProgress,
            priority: Priority::MEDIUM,
            assignee: assignee.map(str::to_string),
            task_type: TaskType::Task,
            epic: None,
            branch: None,
            updated_at: None,
        epic_verification_owner: None,
        }
        }

    fn data(in_progress_tasks: Vec<cas_factory::TaskSummary>) -> DirectorData {
        DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks,
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
            start_gated_task_ids: Default::default(),
        }
    }

    /// cas-eb7f (review finding, cas-ebc1 final): the primary lookup path —
    /// resolving via `agent.current_task` matched against
    /// `data.in_progress_tasks` — was previously untested; the only existing
    /// test always built agents with `current_task: None`, so it exercised
    /// only the assignee-fallback branch. This is the common-case path a
    /// healthy worker hits every render.
    #[test]
    fn find_agent_in_progress_task_resolves_via_current_task_id() {
        let agent = agent("agent-1", "swift-fox", Some("cas-1234"));
        // Deliberately give the matching task a DIFFERENT assignee than the
        // agent's id/name, so the fallback branch could not accidentally
        // produce the same answer — a regression to the fallback-only path
        // would return None here, not silently pass.
        let data = data(vec![task("cas-1234", Some("someone-else"))]);

        let found = find_agent_in_progress_task(&agent, &data);

        assert_eq!(
            found.map(|t| t.id.as_str()),
            Some("cas-1234"),
            "must resolve via agent.current_task, not the assignee fallback"
        );
    }

    /// A stale `current_task` (task no longer in `in_progress_tasks`, e.g.
    /// just closed) must fall through to the assignee-based lookup rather
    /// than returning `None` outright.
    #[test]
    fn find_agent_in_progress_task_falls_back_when_current_task_id_is_stale() {
        let agent = agent("agent-1", "swift-fox", Some("cas-9999"));
        let data = data(vec![task("cas-1234", Some("agent-1"))]);

        let found = find_agent_in_progress_task(&agent, &data);

        assert_eq!(
            found.map(|t| t.id.as_str()),
            Some("cas-1234"),
            "stale current_task must fall through to the assignee match"
        );
    }

    /// Negative control: no `current_task` and no matching assignee → `None`.
    #[test]
    fn find_agent_in_progress_task_returns_none_when_nothing_matches() {
        let agent = agent("agent-1", "swift-fox", None);
        let data = data(vec![task("cas-1234", Some("someone-else"))]);

        assert!(find_agent_in_progress_task(&agent, &data).is_none());
    }
}
