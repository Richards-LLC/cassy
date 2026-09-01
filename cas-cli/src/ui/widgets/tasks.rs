//! Task list widget for sidecar and factory TUI

use std::collections::{HashMap, HashSet};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, ListState};

use cas_types::{Priority, TaskStatus, TaskType};

use crate::ui::theme::{ActiveTheme, Icons, get_agent_color};

use crate::ui::widgets::utils::{priority_color, truncate, truncate_to_width};

/// Display info for a task item
#[derive(Debug, Clone)]
pub struct TaskDisplayInfo {
    pub id: String,
    pub title: String,
    pub priority: Priority,
    pub status: TaskStatus,
    pub task_type: TaskType,
    pub assignee: Option<String>,
}

/// Configuration for task list rendering
#[derive(Debug, Default)]
pub struct TaskConfig {
    /// Map from agent ID to agent name (for color lookup)
    pub agent_id_to_name: HashMap<String, String>,
    /// IDs of currently active agents
    pub active_agent_ids: HashSet<String>,
    /// Whether to show indentation for subtasks
    pub show_indent: bool,
    /// Whether to show priority badge
    pub show_priority: bool,
    /// Whether this panel is focused
    pub focused: bool,
}

impl TaskConfig {
    pub fn new() -> Self {
        Self {
            show_priority: true,
            ..Default::default()
        }
    }

    pub fn with_agent_maps(
        agent_id_to_name: HashMap<String, String>,
        active_agent_ids: HashSet<String>,
    ) -> Self {
        Self {
            agent_id_to_name,
            active_agent_ids,
            show_priority: true,
            show_indent: false,
            focused: false,
        }
    }
}

/// Render a stateless task list (for factory director)
pub fn render_task_list(
    frame: &mut Frame,
    area: Rect,
    tasks: &[TaskDisplayInfo],
    theme: &ActiveTheme,
    config: &TaskConfig,
    title: Option<&str>,
) {
    let palette = &theme.palette;
    let block = if let Some(title) = title {
        Block::default()
            .title(format!(
                " {} ({}/{}) ",
                title,
                tasks
                    .iter()
                    .filter(|t| t.status == TaskStatus::InProgress)
                    .count(),
                tasks.len()
            ))
            .borders(Borders::TOP)
            .border_style(Style::default().fg(palette.border_muted))
    } else {
        Block::default()
    };

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let items = build_task_items(tasks, theme, config, inner.width);
    let list = List::new(items);
    frame.render_widget(list, inner);
}

/// Render a stateful task list (for sidecar with selection)
pub fn render_task_list_with_state(
    frame: &mut Frame,
    area: Rect,
    tasks: &[TaskDisplayInfo],
    theme: &ActiveTheme,
    config: &TaskConfig,
    state: &mut ListState,
    block: Block,
) {
    let styles = &theme.styles;
    let inner = block.inner(area);
    let items = build_task_items(tasks, theme, config, inner.width);
    let list = List::new(items)
        .block(block)
        .highlight_style(styles.bg_selection);
    frame.render_stateful_widget(list, area, state);
}

/// Build task list items
fn build_task_items(
    tasks: &[TaskDisplayInfo],
    theme: &ActiveTheme,
    config: &TaskConfig,
    width: u16,
) -> Vec<ListItem<'static>> {
    let styles = &theme.styles;
    let mut items: Vec<ListItem> = Vec::new();

    for task in tasks {
        items.push(build_task_item(task, theme, config, width, false));
    }

    if items.is_empty() {
        items.push(ListItem::new(Line::from(vec![Span::styled(
            "No tasks",
            styles.text_muted.add_modifier(Modifier::ITALIC),
        )])));
    }

    items
}

/// Build a single task item
pub fn build_task_item(
    task: &TaskDisplayInfo,
    theme: &ActiveTheme,
    config: &TaskConfig,
    width: u16,
    indented: bool,
) -> ListItem<'static> {
    let palette = &theme.palette;
    // Determine color based on assignee (assignees store agent names directly)
    let task_color = task
        .assignee
        .as_ref()
        .map(|name| get_agent_color(name))
        .unwrap_or(palette.text_primary);

    let status_icon = match task.status {
        TaskStatus::InProgress => Icons::SPINNER_STATIC,
        TaskStatus::Open => Icons::CIRCLE_EMPTY,
        TaskStatus::Blocked => Icons::CIRCLE_X,
        TaskStatus::Closed => Icons::CHECK,
        TaskStatus::Cancelled => Icons::CIRCLE_X,
        TaskStatus::AwaitingMerge => Icons::CLOCK,
    };

    let status_color = match task.status {
        TaskStatus::InProgress => palette.task_in_progress,
        TaskStatus::Blocked => palette.task_blocked,
        TaskStatus::Closed => palette.task_closed,
        TaskStatus::Cancelled => palette.task_closed,
        TaskStatus::Open => palette.task_open,
        TaskStatus::AwaitingMerge => palette.task_blocked,
    };

    let indent = if indented { "  " } else { "" };
    let indent_len = if indented { 2 } else { 0 };
    let priority_len = if config.show_priority { 4 } else { 0 };
    let title_truncated = truncate_to_width(
        &task.title,
        width,
        14 + task.id.len() + indent_len + priority_len,
    );

    let mut spans = vec![
        Span::raw(indent.to_string()),
        Span::styled(status_icon.to_string(), Style::default().fg(status_color)),
        Span::raw(" "),
        Span::styled(task.id.clone(), Style::default().fg(task_color)),
        Span::raw(" "),
    ];

    if config.show_priority {
        spans.push(Span::styled(
            format!("P{}", task.priority.0),
            Style::default().fg(priority_color(task.priority, palette)),
        ));
        spans.push(Span::raw(" "));
    }

    spans.push(Span::styled(
        title_truncated,
        Style::default().fg(task_color),
    ));

    ListItem::new(Line::from(spans))
}

/// Render compact task list for factory director (simplified version)
pub fn render_compact_task_list(
    frame: &mut Frame,
    area: Rect,
    in_progress: &[TaskDisplayInfo],
    ready: &[TaskDisplayInfo],
    theme: &ActiveTheme,
    config: &TaskConfig,
) {
    let palette = &theme.palette;
    let styles = &theme.styles;
    let border_color = if config.focused {
        palette.border_focused
    } else {
        palette.border_muted
    };

    let block = Block::default()
        .title(format!(
            " Tasks ({}/{}) ",
            in_progress.len(),
            in_progress.len() + ready.len()
        ))
        .borders(Borders::TOP)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let mut items: Vec<ListItem> = Vec::new();

    // In-progress tasks first
    for task in in_progress {
        // Task assignees store agent names directly (not IDs)
        let agent_color = task
            .assignee
            .as_ref()
            .map(|name| get_agent_color(name))
            .unwrap_or(palette.task_in_progress);

        let title = truncate(&task.title, inner.width.saturating_sub(12) as usize);
        items.push(ListItem::new(Line::from(vec![
            Span::styled(
                Icons::SPINNER_STATIC.to_string(),
                Style::default().fg(palette.task_in_progress),
            ),
            Span::raw(" "),
            Span::styled(task.id.clone(), Style::default().fg(agent_color)),
            Span::raw(" "),
            Span::styled(title, Style::default().fg(agent_color)),
        ])));
    }

    // Ready tasks
    for task in ready {
        let title = truncate(&task.title, inner.width.saturating_sub(12) as usize);
        let priority_color = priority_color(task.priority, palette);

        items.push(ListItem::new(Line::from(vec![
            Span::styled(
                Icons::CIRCLE_EMPTY.to_string(),
                Style::default().fg(palette.task_open),
            ),
            Span::raw(" "),
            Span::styled(task.id.clone(), Style::default().fg(palette.text_primary)),
            Span::raw(" "),
            Span::styled(
                format!("P{}", task.priority.0),
                Style::default().fg(priority_color),
            ),
            Span::raw(" "),
            Span::styled(title, Style::default().fg(palette.text_primary)),
        ])));
    }

    if items.is_empty() {
        items.push(ListItem::new(Line::from(vec![Span::styled(
            "No tasks",
            styles.text_muted.add_modifier(Modifier::ITALIC),
        )])));
    }

    let list = List::new(items);
    frame.render_widget(list, inner);
}
