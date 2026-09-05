//! Sidecar panels - displays Cassy tasks, agents, changes, and activity
//!
//! This module provides native ratatui widgets for the factory TUI sidecar.
//! The panels are rendered directly without a containing block wrapper,
//! each section has its own header via the compact widget functions.

pub(crate) mod activity;
pub mod agent_helpers;
pub(crate) mod changes;
mod data;
mod events;
mod factory_radar;
pub mod mission_epic;
pub mod mission_workers;
pub mod panel;
mod prompts;
mod reminders;
#[cfg(test)]
mod supervisor_stall_tests;
pub(crate) mod tasks;

pub use data::{AgentSummary, DirectorData, DirectorStores, TaskSummary};
pub(crate) use events::pick_best_open_branch_epic;
pub use events::{DirectorEvent, DirectorEventDetector};
// cas-893c: shared idle-confidence gate, reused by the daemon's delivery-time
// idle-nudge decision (queue_and_events.rs) so the "is this worker really
// idle, not just between turns" heuristic has one definition.
pub(crate) use events::{
    effective_stall_threshold_secs, FRESH_HEARTBEAT_SECS, RECENT_ACTIVITY_SECS,
};
pub use panel::PanelRegistry;
pub use prompts::{
    check_merge_alert_freshness, check_merge_alert_freshness_for_task, compute_gated_task_ids,
    generate_prompt_at, revalidate_event_for_delivery_with_context,
    revalidate_event_for_delivery_with_focus, with_response_instructions,
    worker_now_has_real_assignment, MergeAlertFreshness, Prompt,
};
pub(crate) use prompts::{epic_completion_is_current, prompt_is_still_deliverable};
// PanelAreas, SidecarFocus, SidecarState, ViewMode, DiffLine, DiffLineType, render, render_with_state are already public in this module

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::widgets::ListState;
use ratatui::Frame;

use crate::ui::theme::ActiveTheme;
use crate::ui::widgets::TreeItemType;

/// Which sidecar panel has focus
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub enum SidecarFocus {
    #[default]
    None,
    Factory,
    Tasks,
    Reminders,
    Changes,
    Activity,
}

/// View mode for the sidecar panel
#[derive(Debug, Clone, PartialEq, Default)]
pub enum ViewMode {
    /// Overview showing all panels
    #[default]
    Overview,
    /// Full task detail view
    TaskDetail(String),
    /// Full activity log view
    ActivityLog,
    /// File diff view (source_path, file_path)
    FileDiff(std::path::PathBuf, String),
}

/// Type of diff line for coloring
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DiffLineType {
    Context,
    Added,
    Removed,
    HunkHeader,
    FileHeader,
}

/// A processed diff line with line numbers
#[derive(Debug, Clone)]
pub struct DiffLine {
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub content: String,
    pub line_type: DiffLineType,
}

impl SidecarFocus {
    /// Cycle to the next panel
    pub fn next(self) -> Self {
        match self {
            Self::None => Self::Factory,
            Self::Factory => Self::Tasks,
            Self::Tasks => Self::Reminders,
            Self::Reminders => Self::Changes,
            Self::Changes => Self::Activity,
            Self::Activity => Self::None,
        }
    }

    /// Cycle to the previous panel
    pub fn prev(self) -> Self {
        match self {
            Self::None => Self::Activity,
            Self::Factory => Self::None,
            Self::Tasks => Self::Factory,
            Self::Reminders => Self::Tasks,
            Self::Changes => Self::Reminders,
            Self::Activity => Self::Changes,
        }
    }

    /// Cycle to the next panel, skipping Reminders if there are none
    pub fn next_with_reminders(self, has_reminders: bool) -> Self {
        let next = self.next();
        if next == Self::Reminders && !has_reminders {
            next.next()
        } else {
            next
        }
    }

    /// Cycle to the previous panel, skipping Reminders if there are none
    pub fn prev_with_reminders(self, has_reminders: bool) -> Self {
        let prev = self.prev();
        if prev == Self::Reminders && !has_reminders {
            prev.prev()
        } else {
            prev
        }
    }
}

/// Mutable state for sidecar rendering
pub struct SidecarState<'a> {
    pub focus: SidecarFocus,
    pub tasks_state: &'a mut ListState,
    pub agents_state: &'a mut ListState,
    pub reminders_state: &'a mut ListState,
    pub changes_state: &'a mut ListState,
    pub activity_state: &'a mut ListState,
    /// Optional agent filter (filter tasks/activity by this agent name)
    pub agent_filter: Option<&'a str>,
    /// Current display-focused epic ID.
    pub focused_epic_id: Option<&'a str>,
    /// Cached branch/ahead-behind status for the current display-focused epic.
    pub focused_epic_branch_status: Option<EpicBranchStatus<'a>>,
    /// Section collapse flags
    pub factory_collapsed: bool,
    pub tasks_collapsed: bool,
    pub reminders_collapsed: bool,
    pub changes_collapsed: bool,
    pub activity_collapsed: bool,
    /// Collapsed epic IDs
    pub collapsed_epics: &'a std::collections::HashSet<String>,
    /// Collapsed directory paths in changes panel
    pub collapsed_dirs: &'a std::collections::HashSet<String>,
    /// Output: tree item types from changes panel (for scroll bounds)
    pub changes_item_types: &'a mut Vec<TreeItemType>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EpicBranchStatus<'a> {
    pub branch: &'a str,
    pub ahead: u32,
    pub behind: u32,
}

/// Panel areas for click detection
#[derive(Debug, Clone, Copy, Default)]
#[allow(dead_code)]
pub struct PanelAreas {
    pub factory: Rect,
    pub tasks: Rect,
    pub reminders: Rect,
    pub changes: Rect,
    pub activity: Rect,
}

/// Render the sidecar panels with optional navigation state
///
/// Returns the panel areas for click detection.
pub fn render_with_state(
    frame: &mut Frame,
    area: Rect,
    data: &DirectorData,
    theme: &ActiveTheme,
    focused_epic_id: Option<&str>,
    supervisor_name: &str,
    activity_backend_tags: &std::collections::HashMap<String, &'static str>,
    mut state: Option<&mut SidecarState>,
) -> PanelAreas {
    let factory_collapsed = state.as_ref().map(|s| s.factory_collapsed).unwrap_or(false);
    // Get collapse flags
    let tasks_collapsed = state.as_ref().map(|s| s.tasks_collapsed).unwrap_or(false);
    let reminders_collapsed = state
        .as_ref()
        .map(|s| s.reminders_collapsed)
        .unwrap_or(false);
    let changes_collapsed = state.as_ref().map(|s| s.changes_collapsed).unwrap_or(false);
    let activity_collapsed = state
        .as_ref()
        .map(|s| s.activity_collapsed)
        .unwrap_or(false);

    let focus = state
        .as_ref()
        .map(|s| s.focus)
        .unwrap_or(SidecarFocus::None);
    tracing::debug!("render_with_state: focus={:?}, area={:?}", focus, area);

    let agent_filter = state.as_ref().and_then(|s| s.agent_filter);
    let focused_epic_id = state
        .as_ref()
        .and_then(|s| s.focused_epic_id)
        .or(focused_epic_id);
    #[allow(clippy::incompatible_msrv)]
    static EMPTY_SET: std::sync::LazyLock<std::collections::HashSet<String>> =
        std::sync::LazyLock::new(std::collections::HashSet::new);
    let collapsed_epics = state
        .as_ref()
        .map(|s| s.collapsed_epics)
        .unwrap_or(&EMPTY_SET);

    let has_reminders = !data.reminders.is_empty();
    let has_changes = data.changes.iter().any(|source| !source.changes.is_empty());
    // cas-eb7f: build ScopedTaskView exactly once per frame and thread it into
    // both the collapse decision below and the render call further down —
    // previously tasks::render_with_focus rebuilt the identical clone-heavy
    // view a second time (data.tasks_by_epic() clones every task/epic).
    let scoped_task_view = tasks::ScopedTaskView::new(data, focused_epic_id);
    let visible_task_rows = scoped_task_view.visible_row_count(agent_filter, collapsed_epics);
    // cas-eb7f: `visible_row_count()` counts only `epic_groups`/`standalone`,
    // which are empty by construction whenever unfocused (ScopedTaskView::new
    // clears epic_groups to Vec::new() when focused_epic_id is None). Without
    // this guard, the cas-6945 "no epic focused — live epics: ..." hint would
    // never be seen: `visible_task_rows == 0` would auto-collapse the panel to
    // a 1-line header, and render_with_focus returns before ever reaching the
    // hint's rendering when `collapsed` is true — silently reproducing the
    // exact "active task went invisible" regression cas-6945 was filed for.
    let has_unfocused_hint = !scoped_task_view.unfocused_live_epics.is_empty();
    let effective_tasks_collapsed =
        tasks_collapsed || (visible_task_rows == 0 && !has_unfocused_hint);
    let effective_reminders_collapsed = reminders_collapsed || !has_reminders;
    let effective_changes_collapsed = changes_collapsed || !has_changes;

    // Calculate constraints based on collapse state (collapsed = 1 line header only)
    // Empty sections render as one-line headers without mutating manual collapse state.
    let mut constraints: Vec<Constraint> = vec![
        if factory_collapsed {
            Constraint::Length(1)
        } else {
            Constraint::Percentage(if has_reminders { 25 } else { 28 })
        },
        if effective_tasks_collapsed {
            Constraint::Length(1)
        } else {
            Constraint::Percentage(if has_reminders { 23 } else { 26 })
        },
    ];

    constraints.push(if effective_reminders_collapsed {
        Constraint::Length(1)
    } else {
        Constraint::Percentage(14)
    });

    constraints.push(if effective_changes_collapsed {
        Constraint::Length(1)
    } else {
        Constraint::Percentage(if has_reminders { 19 } else { 23 })
    });
    constraints.push(if activity_collapsed {
        Constraint::Length(1)
    } else {
        Constraint::Min(0)
    });

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(area);

    let focused_epic_branch_status = state.as_ref().and_then(|s| s.focused_epic_branch_status);

    // Track chunk indices
    let factory_idx = 0;
    let tasks_idx = 1;
    let reminders_idx = 2;
    let changes_idx = 3;
    let activity_idx = 4;

    // Render each section with focus indicator and collapse state
    factory_radar::render_with_focus(
        frame,
        chunks[factory_idx],
        data,
        theme,
        focused_epic_id,
        focused_epic_branch_status,
        focus == SidecarFocus::Factory,
        state.as_ref().and_then(|s| s.agents_state.selected()),
        supervisor_name,
        factory_collapsed,
    );
    tasks::render_with_focus(
        frame,
        chunks[tasks_idx],
        data,
        theme,
        focus == SidecarFocus::Tasks,
        state.as_ref().and_then(|s| s.tasks_state.selected()),
        agent_filter,
        focused_epic_id,
        &scoped_task_view,
        effective_tasks_collapsed,
        collapsed_epics,
        state.as_mut().map(|s| &mut *s.tasks_state),
    );

    reminders::render_with_focus(
        frame,
        chunks[reminders_idx],
        data,
        theme,
        focus == SidecarFocus::Reminders,
        effective_reminders_collapsed,
        state.as_mut().map(|s| &mut *s.reminders_state),
    );
    let reminders_area = chunks[reminders_idx];

    // Get collapsed_dirs from state (or empty set if no state)
    let collapsed_dirs = state
        .as_ref()
        .map(|s| s.collapsed_dirs)
        .unwrap_or(&EMPTY_SET);

    let item_types = changes::render_with_focus(
        frame,
        chunks[changes_idx],
        data,
        theme,
        focus == SidecarFocus::Changes,
        state.as_ref().and_then(|s| s.changes_state.selected()),
        effective_changes_collapsed,
        state.as_mut().map(|s| &mut *s.changes_state),
        collapsed_dirs,
    );
    // Store item types for scroll bounds calculation
    if let Some(ref mut s) = state {
        *s.changes_item_types = item_types;
    }
    activity::render_with_focus(
        frame,
        chunks[activity_idx],
        data,
        theme,
        focus == SidecarFocus::Activity,
        state.as_ref().and_then(|s| s.activity_state.selected()),
        activity_collapsed,
        activity_backend_tags,
    );

    // Return panel areas for click detection
    PanelAreas {
        factory: chunks[factory_idx],
        tasks: chunks[tasks_idx],
        reminders: reminders_area,
        changes: chunks[changes_idx],
        activity: chunks[activity_idx],
    }
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use cas_factory::{DirectorData, TaskSummary};
    use cas_types::{Priority, TaskStatus, TaskType};
    use ratatui::backend::TestBackend;
    use ratatui::widgets::ListState;
    use ratatui::Terminal;

    use crate::ui::theme::ActiveTheme;

    use super::{render_with_state, SidecarFocus, SidecarState};

    fn task(id: &str, task_type: TaskType, epic: Option<&str>) -> TaskSummary {
        TaskSummary {
            id: id.to_string(),
            title: id.to_string(),
            status: TaskStatus::Open,
            priority: Priority::MEDIUM,
            assignee: None,
            task_type,
            epic: epic.map(str::to_string),
            branch: Some(format!("epic/{id}")).filter(|_| task_type == TaskType::Epic),
            updated_at: None,
            epic_verification_owner: None,
        }
    }

    fn data_with_two_epics() -> DirectorData {
        DirectorData {
            ready_tasks: vec![
                task("cas-state-child", TaskType::Task, Some("cas-state")),
                task("cas-param-child", TaskType::Task, Some("cas-param")),
            ],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![
                task("cas-state", TaskType::Epic, None),
                task("cas-param", TaskType::Epic, None),
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        }
    }

    fn empty_data() -> DirectorData {
        DirectorData {
            ready_tasks: Vec::new(),
            in_progress_tasks: Vec::new(),
            epic_tasks: Vec::new(),
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        }
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<Vec<_>>()
            .join("")
    }

    #[test]
    fn render_with_state_prefers_state_focused_epic_id_over_parameter() {
        let data = data_with_two_epics();
        let backend = TestBackend::new(90, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();
        let mut tasks_state = ListState::default();
        let mut agents_state = ListState::default();
        let mut reminders_state = ListState::default();
        let mut changes_state = ListState::default();
        let mut activity_state = ListState::default();
        let collapsed_epics = HashSet::new();
        let collapsed_dirs = HashSet::new();
        let mut changes_item_types = Vec::new();

        terminal
            .draw(|frame| {
                let mut state = SidecarState {
                    focus: SidecarFocus::Factory,
                    tasks_state: &mut tasks_state,
                    agents_state: &mut agents_state,
                    reminders_state: &mut reminders_state,
                    changes_state: &mut changes_state,
                    activity_state: &mut activity_state,
                    agent_filter: None,
                    focused_epic_id: Some("cas-state"),
                    focused_epic_branch_status: None,
                    factory_collapsed: false,
                    tasks_collapsed: false,
                    reminders_collapsed: false,
                    changes_collapsed: false,
                    activity_collapsed: false,
                    collapsed_epics: &collapsed_epics,
                    collapsed_dirs: &collapsed_dirs,
                    changes_item_types: &mut changes_item_types,
                };
                render_with_state(
                    frame,
                    frame.area(),
                    &data,
                    &theme,
                    Some("cas-param"),
                    "supervisor",
                    &std::collections::HashMap::new(),
                    Some(&mut state),
                );
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("EPIC: cas-state"));
        assert!(!text.contains("EPIC: cas-param"));
    }

    #[test]
    fn empty_sidecar_sections_collapse_to_headers_and_activity_gains_space() {
        let data = empty_data();
        let backend = TestBackend::new(90, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();
        let mut tasks_state = ListState::default();
        let mut agents_state = ListState::default();
        let mut reminders_state = ListState::default();
        let mut changes_state = ListState::default();
        let mut activity_state = ListState::default();
        let collapsed_epics = HashSet::new();
        let collapsed_dirs = HashSet::new();
        let mut changes_item_types = Vec::new();
        let mut areas = super::PanelAreas::default();

        terminal
            .draw(|frame| {
                let mut state = SidecarState {
                    focus: SidecarFocus::None,
                    tasks_state: &mut tasks_state,
                    agents_state: &mut agents_state,
                    reminders_state: &mut reminders_state,
                    changes_state: &mut changes_state,
                    activity_state: &mut activity_state,
                    agent_filter: None,
                    focused_epic_id: None,
                    focused_epic_branch_status: None,
                    factory_collapsed: false,
                    tasks_collapsed: false,
                    reminders_collapsed: false,
                    changes_collapsed: false,
                    activity_collapsed: false,
                    collapsed_epics: &collapsed_epics,
                    collapsed_dirs: &collapsed_dirs,
                    changes_item_types: &mut changes_item_types,
                };
                areas = render_with_state(
                    frame,
                    frame.area(),
                    &data,
                    &theme,
                    None,
                    "supervisor",
                    &std::collections::HashMap::new(),
                    Some(&mut state),
                );
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("▸ TASKS (0)"));
        assert!(text.contains("▸ REMINDERS (0)"));
        assert!(text.contains("▸ CHANGES (0)"));
        assert_eq!(areas.tasks.height, 1);
        assert_eq!(areas.reminders.height, 1);
        assert_eq!(areas.changes.height, 1);
        assert!(areas.activity.height > areas.factory.height);
    }

    /// cas-eb7f: `visible_row_count()` counts only `epic_groups`/`standalone`,
    /// both empty by construction whenever unfocused — so a naive
    /// `visible_task_rows == 0` auto-collapse would hide the cas-6945
    /// "no epic focused — live epics: ..." hint behind a 1-line "TASKS (0)"
    /// header, reproducing the exact "active task went invisible" bug
    /// cas-6945 was filed to fix. A live epic with no session-owned
    /// standalone task (so `visible_task_rows` is genuinely 0) must still
    /// leave the panel expanded so the hint renders.
    #[test]
    fn render_with_state_does_not_auto_collapse_tasks_when_unfocused_hint_has_content() {
        let data = DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "cas-live-child".to_string(),
                title: "Live child".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                // cas-6185c: must be session-visible (unassigned, here) —
                // the unfocused hint now applies the same privacy gate as
                // the FACTORY-panel overview, so an assignee outside
                // `agent_id_to_name` would be correctly filtered out as a
                // foreign epic, leaving the hint (and this test) with
                // nothing to show.
                assignee: None,
                task_type: TaskType::Task,
                epic: Some("cas-live".to_string()),
                branch: None,
                updated_at: None,
                epic_verification_owner: None,
            }],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![task("cas-live", TaskType::Epic, None)],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        };
        let backend = TestBackend::new(90, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();
        let mut tasks_state = ListState::default();
        let mut agents_state = ListState::default();
        let mut reminders_state = ListState::default();
        let mut changes_state = ListState::default();
        let mut activity_state = ListState::default();
        let collapsed_epics = HashSet::new();
        let collapsed_dirs = HashSet::new();
        let mut changes_item_types = Vec::new();
        let mut areas = super::PanelAreas::default();

        terminal
            .draw(|frame| {
                let mut state = SidecarState {
                    focus: SidecarFocus::None,
                    tasks_state: &mut tasks_state,
                    agents_state: &mut agents_state,
                    reminders_state: &mut reminders_state,
                    changes_state: &mut changes_state,
                    activity_state: &mut activity_state,
                    agent_filter: None,
                    focused_epic_id: None,
                    focused_epic_branch_status: None,
                    factory_collapsed: false,
                    tasks_collapsed: false,
                    reminders_collapsed: false,
                    changes_collapsed: false,
                    activity_collapsed: false,
                    collapsed_epics: &collapsed_epics,
                    collapsed_dirs: &collapsed_dirs,
                    changes_item_types: &mut changes_item_types,
                };
                areas = render_with_state(
                    frame,
                    frame.area(),
                    &data,
                    &theme,
                    None,
                    "supervisor",
                    &std::collections::HashMap::new(),
                    Some(&mut state),
                );
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(
            !text.contains("▸ TASKS (0)"),
            "panel must not auto-collapse to a 1-line header while the \
             unfocused-live-epics hint has content to show; got:\n{text}"
        );
        assert!(
            areas.tasks.height > 1,
            "collapsed-header height (1) means the hint never rendered"
        );
        assert!(text.contains("No epic focused"));
        assert!(text.contains("cas-live"));
        assert!(text.contains("focus_epic"));
    }

    #[test]
    fn manual_sidecar_collapse_still_overrides_non_empty_sections() {
        let data = data_with_two_epics();
        let backend = TestBackend::new(90, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();
        let mut tasks_state = ListState::default();
        let mut agents_state = ListState::default();
        let mut reminders_state = ListState::default();
        let mut changes_state = ListState::default();
        let mut activity_state = ListState::default();
        let collapsed_epics = HashSet::new();
        let collapsed_dirs = HashSet::new();
        let mut changes_item_types = Vec::new();
        let mut areas = super::PanelAreas::default();

        terminal
            .draw(|frame| {
                let mut state = SidecarState {
                    focus: SidecarFocus::Tasks,
                    tasks_state: &mut tasks_state,
                    agents_state: &mut agents_state,
                    reminders_state: &mut reminders_state,
                    changes_state: &mut changes_state,
                    activity_state: &mut activity_state,
                    agent_filter: None,
                    focused_epic_id: Some("cas-state"),
                    focused_epic_branch_status: None,
                    factory_collapsed: false,
                    tasks_collapsed: true,
                    reminders_collapsed: false,
                    changes_collapsed: false,
                    activity_collapsed: false,
                    collapsed_epics: &collapsed_epics,
                    collapsed_dirs: &collapsed_dirs,
                    changes_item_types: &mut changes_item_types,
                };
                areas = render_with_state(
                    frame,
                    frame.area(),
                    &data,
                    &theme,
                    None,
                    "supervisor",
                    &std::collections::HashMap::new(),
                    Some(&mut state),
                );
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("▸ TASKS (2)"));
        assert_eq!(areas.tasks.height, 1);
    }
}
