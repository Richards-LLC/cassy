//! Mission Control: Status Strip
//!
//! Compact single-row strip showing epic progress, task counts, and worker status chips.

use cas_types::TaskStatus;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::DirectorData;
use super::agent_helpers::count_agent_statuses;
use crate::ui::theme::{ActiveTheme, Icons};

/// Render the status strip into a single-row area.
///
/// Layout: `cas-a1b2 ████░░ 60%  Done:4 Active:2 Queue:1 Blocked:0  ●3 ◐1 ⊘0`
pub fn render_status_strip(
    frame: &mut Frame,
    area: Rect,
    data: &DirectorData,
    theme: &ActiveTheme,
    focused_epic_id: Option<&str>,
) {
    let palette = &theme.palette;
    let styles = &theme.styles;

    if area.width < 10 || area.height == 0 {
        return;
    }

    let mut left_spans: Vec<Span> = Vec::new();

    let active_epic =
        focused_epic_id.and_then(|epic_id| data.epic_tasks.iter().find(|e| e.id == epic_id));

    if let Some(epic) = active_epic {
        // Count subtask statuses from live task lists
        let live_tasks: Vec<_> = data
            .in_progress_tasks
            .iter()
            .chain(data.ready_tasks.iter())
            .filter(|t| t.epic.as_ref() == Some(&epic.id))
            .collect();

        let done = data.epic_closed_counts.get(&epic.id).copied().unwrap_or(0);
        let active = live_tasks
            .iter()
            .filter(|t| t.status == TaskStatus::InProgress)
            .count();
        let blocked = live_tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Blocked)
            .count();
        let queue = live_tasks
            .iter()
            .filter(|t| t.status == TaskStatus::Open)
            .count();
        let total = done + live_tasks.len();

        // Epic ID
        left_spans.push(Span::styled(
            format!(" {} ", epic.id),
            styles.text_info.add_modifier(Modifier::BOLD),
        ));

        // Progress bar (compact)
        let bar_width = 12usize.min((area.width as usize).saturating_sub(60));
        if bar_width > 2 {
            let pct = if total > 0 { (done * 100) / total } else { 0 };
            let filled = if total > 0 {
                (done * bar_width) / total
            } else {
                0
            };
            let in_progress_width = if total > 0 {
                (active * bar_width) / total
            } else {
                0
            };
            let empty = bar_width.saturating_sub(filled + in_progress_width);

            left_spans.push(Span::styled(
                Icons::PROGRESS_FULL.repeat(filled),
                Style::default().fg(palette.task_closed),
            ));
            left_spans.push(Span::styled(
                Icons::PROGRESS_MEDIUM.repeat(in_progress_width),
                Style::default().fg(palette.task_in_progress),
            ));
            left_spans.push(Span::styled(
                Icons::PROGRESS_EMPTY.repeat(empty),
                styles.text_muted,
            ));
            left_spans.push(Span::styled(format!(" {pct}%"), styles.text_primary));
        }

        // Task counts
        left_spans.push(Span::styled("  ", styles.text_muted));
        left_spans.push(Span::styled(
            format!("{done}"),
            Style::default().fg(palette.task_closed),
        ));
        left_spans.push(Span::styled(" done ", styles.text_muted));
        left_spans.push(Span::styled(
            format!("{active}"),
            Style::default().fg(palette.task_in_progress),
        ));
        left_spans.push(Span::styled(" active ", styles.text_muted));
        left_spans.push(Span::styled(
            format!("{queue}"),
            Style::default().fg(palette.task_open),
        ));
        left_spans.push(Span::styled(" queue", styles.text_muted));
        if blocked > 0 {
            left_spans.push(Span::styled(" ", styles.text_muted));
            left_spans.push(Span::styled(
                format!("{blocked}"),
                Style::default().fg(palette.task_blocked),
            ));
            left_spans.push(Span::styled(" blocked", styles.text_muted));
        }
    } else {
        left_spans.push(Span::styled(" No active epic", styles.text_muted));
    }

    // Right side: worker status chips
    let status_counts = count_agent_statuses(&data.agents);
    let active_count = status_counts.active;
    let idle_count = status_counts.idle;
    let dead_count = status_counts.dead;

    let mut right_spans: Vec<Span> = Vec::new();
    right_spans.push(Span::styled(
        format!("{} {active_count}", Icons::CIRCLE_FILLED),
        Style::default().fg(palette.agent_active),
    ));
    right_spans.push(Span::styled(" ", styles.text_muted));
    right_spans.push(Span::styled(
        format!("{} {idle_count}", Icons::CIRCLE_HALF),
        Style::default().fg(palette.agent_idle),
    ));
    if dead_count > 0 {
        right_spans.push(Span::styled(" ", styles.text_muted));
        right_spans.push(Span::styled(
            format!("\u{2298} {dead_count}"),
            Style::default().fg(palette.agent_dead),
        ));
    }
    right_spans.push(Span::raw(" "));

    // Build the line with padding between left and right
    let left_width: usize = left_spans.iter().map(|s| s.width()).sum();
    let right_width: usize = right_spans.iter().map(|s| s.width()).sum();
    let padding = (area.width as usize).saturating_sub(left_width + right_width);

    let mut all_spans = left_spans;
    all_spans.push(Span::raw(" ".repeat(padding)));
    all_spans.extend(right_spans);

    let paragraph = Paragraph::new(Line::from(all_spans)).style(styles.bg_elevated);
    frame.render_widget(paragraph, area);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use cas_factory::{DirectorData, TaskSummary};
    use cas_types::{Priority, TaskStatus, TaskType};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::ui::theme::ActiveTheme;

    use super::render_status_strip;

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

    fn data_with_unassigned_subtask() -> DirectorData {
        DirectorData {
            ready_tasks: vec![task("cas-child", TaskType::Task, Some("cas-focused"))],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![task("cas-focused", TaskType::Epic, None)],
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
    fn status_strip_renders_no_active_epic_without_resolved_focus() {
        let data = data_with_unassigned_subtask();
        let backend = TestBackend::new(90, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();

        terminal
            .draw(|frame| {
                render_status_strip(frame, frame.area(), &data, &theme, None);
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("No active epic"));
        assert!(!text.contains("cas-focused"));
    }

    #[test]
    fn status_strip_renders_resolved_focus_with_unassigned_subtasks() {
        let data = data_with_unassigned_subtask();
        let backend = TestBackend::new(90, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();

        terminal
            .draw(|frame| {
                render_status_strip(frame, frame.area(), &data, &theme, Some("cas-focused"));
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("cas-focused"));
        assert!(!text.contains("No active epic"));
    }
}
