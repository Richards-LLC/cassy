//! Factory dashboard panel - summary view of factory state
//!
//! Shows epic progress, worker status, and queue at a glance.

use cas_types::{EventType, TaskStatus};
use ratatui::Frame;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};

use crate::ui::factory::director::EpicBranchStatus;
use crate::ui::factory::director::agent_helpers;
use crate::ui::factory::director::data::DirectorData;
use crate::ui::theme::{ActiveTheme, Icons};
use crate::ui::widgets::truncate_to_width;

/// Render the factory dashboard with optional focus indicator
#[allow(clippy::too_many_arguments)]
pub fn render_with_focus(
    frame: &mut Frame,
    area: Rect,
    data: &DirectorData,
    theme: &ActiveTheme,
    focused_epic_id: Option<&str>,
    focused_epic_branch_status: Option<EpicBranchStatus<'_>>,
    focused: bool,
    selected_agent: Option<usize>,
    supervisor_name: &str,
    collapsed: bool,
) {
    let styles = &theme.styles;

    let missing_supervisor = !supervisor_name.is_empty()
        && !data
            .agents
            .iter()
            .any(|agent| agent.name == supervisor_name);
    let agent_count = data.agents.len() + usize::from(missing_supervisor);

    // Collapsed view: single line header
    if collapsed {
        super::panel::render_collapsed_header(
            frame,
            area,
            styles,
            super::panel::CollapsedHeader {
                title: "FACTORY",
                count: agent_count,
                hotkey: Some("f"),
                focused,
                icon_style: None,
            },
        );
        return;
    }

    // Full view with border
    let border_style = if focused {
        styles.border_focused
    } else {
        styles.border_default
    };

    let focus_marker = if focused { "▶" } else { " " };
    let title = format!(" {focus_marker} FACTORY ({agent_count}) ");
    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    if inner.width < 10 || inner.height < 3 {
        return;
    }

    // cas-6185c AC2: decide once whether this frame needs the unfocused
    // live-epics overview data — either because nothing is focused, or
    // because the focused epic fails the source-blind/existence check and
    // `render_epic_progress` falls back to the overview below. Computing
    // `unfocused_live_epic_groups` (which rebuilds the clone-heavy
    // `tasks_by_epic()` grouping) exactly once here, instead of separately
    // inside the sizing branch AND the render branch, is the fix — same
    // pattern cas-eb7f already applied to the TASKS panel.
    let focused_epic_is_renderable = focused_epic_id.is_some_and(|id| {
        crate::ui::factory::director::tasks::epic_is_renderable_source_blind(data, id)
            && data.epic_tasks.iter().any(|e| e.id == id)
    });
    let live_groups: Vec<cas_factory::EpicGroup> = if focused_epic_is_renderable {
        Vec::new()
    } else {
        unfocused_live_epic_groups(data)
    };

    // Layout: epic progress/placeholder, worker list, summary
    //
    // cas-582d: unfocused used to get a fixed 2-line placeholder no matter
    // how much there was to show. It now renders a live-epics overview
    // (header + one row per live epic + pin-hint footer), so give it room
    // scaled to that content — capped so the worker list, the other primary
    // orientation source, never starves.
    let epic_height = if focused_epic_is_renderable {
        if focused_epic_branch_status.is_some() { 3 } else { 2 }
    } else {
        unfocused_epic_progress_height(&live_groups, inner.height)
    };
    let summary_height = 1;
    let worker_height = inner
        .height
        .saturating_sub(epic_height + summary_height + 1); // +1 for separator

    let constraints = vec![
        Constraint::Length(epic_height),
        Constraint::Length(1), // separator
        Constraint::Length(worker_height),
        Constraint::Length(summary_height),
    ];

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints(constraints)
        .split(inner);

    let mut chunk_idx = 0;

    // Epic progress or explicit unfocused placeholder
    render_epic_progress(
        frame,
        chunks[chunk_idx],
        data,
        theme,
        focused_epic_id,
        focused_epic_branch_status,
        &live_groups,
    );
    chunk_idx += 1;

    // Separator
    let sep = Line::from(Span::styled(
        "─".repeat(inner.width as usize),
        styles.text_muted,
    ));
    frame.render_widget(Paragraph::new(sep), chunks[chunk_idx]);
    chunk_idx += 1;

    // Worker list
    render_worker_list(
        frame,
        chunks[chunk_idx],
        data,
        theme,
        selected_agent,
        supervisor_name,
    );
    chunk_idx += 1;

    // Summary bar
    render_summary_bar(frame, chunks[chunk_idx], data, theme, supervisor_name);
}

/// Render epic status bar
///
/// `live_groups` is the pre-computed unfocused-overview data (cas-6185c
/// AC2) — built once by the caller (`render_with_focus`) via
/// `unfocused_live_epic_groups` and threaded through here rather than
/// recomputed. Only consulted on the unfocused/fallback paths below; the
/// normal focused-and-renderable path ignores it entirely.
#[allow(clippy::too_many_arguments)]
fn render_epic_progress(
    frame: &mut Frame,
    area: Rect,
    data: &DirectorData,
    theme: &ActiveTheme,
    focused_epic_id: Option<&str>,
    focused_epic_branch_status: Option<EpicBranchStatus<'_>>,
    live_groups: &[cas_factory::EpicGroup],
) {
    if area.height == 0 || area.width < 10 {
        return;
    }

    let styles = &theme.styles;
    let palette = &theme.palette;

    let Some(focused_epic_id) = focused_epic_id else {
        render_unfocused_overview(frame, area, live_groups, theme);
        return;
    };

    if !crate::ui::factory::director::tasks::epic_is_renderable_source_blind(data, focused_epic_id) {
        render_unfocused_overview(frame, area, live_groups, theme);
        return;
    }

    let Some(epic) = data.epic_tasks.iter().find(|e| e.id == focused_epic_id) else {
        render_unfocused_overview(frame, area, live_groups, theme);
        return;
    };

    // Count tasks for this epic
    let in_progress_count = data
        .in_progress_tasks
        .iter()
        .filter(|t| t.epic.as_ref() == Some(&epic.id))
        .count();

    let queued_count = data
        .ready_tasks
        .iter()
        .filter(|t| t.epic.as_ref() == Some(&epic.id))
        .count();

    let total_visible = in_progress_count + queued_count;

    // Line 1: Epic title with status
    let epic_label = format!("EPIC: {}", epic.id);
    let status_indicator = if epic.status == TaskStatus::InProgress {
        format!(" {}", Icons::TRIANGLE_RIGHT)
    } else {
        String::new()
    };

    let line1 = Line::from(vec![
        Span::styled(epic_label, styles.text_info.add_modifier(Modifier::BOLD)),
        Span::styled(
            status_indicator,
            Style::default().fg(palette.status_success),
        ),
    ]);

    let branch_line = focused_epic_branch_status.map(|status| {
        Line::from(Span::styled(
            format_branch_status(status, area.width),
            styles.text_muted,
        ))
    });

    // Task counts with visual indicator
    // Show: "▓▓▓░░░░░  3 active, 5 queued"
    let bar_width = (area.width as usize).saturating_sub(22).max(4); // Space for counts
    let active_width = if total_visible > 0 {
        ((in_progress_count as f32 / total_visible as f32) * bar_width as f32).round() as usize
    } else {
        0
    };
    let queued_width = bar_width.saturating_sub(active_width);

    let bar = format!(
        "{}{}",
        Icons::PROGRESS_MEDIUM.repeat(active_width),
        Icons::PROGRESS_EMPTY.repeat(queued_width)
    );

    let counts = format!(" {in_progress_count} active, {queued_count} queued");

    let progress_line = Line::from(vec![
        Span::styled(bar, Style::default().fg(palette.agent_active)),
        Span::styled(counts, styles.text_muted),
    ]);

    let lines = if area.height >= 3 {
        if let Some(branch_line) = branch_line {
            vec![line1, branch_line, progress_line]
        } else {
            vec![line1, progress_line]
        }
    } else if area.height >= 2 {
        vec![line1, progress_line]
    } else {
        vec![line1]
    };

    frame.render_widget(Paragraph::new(lines), area);
}

fn format_branch_status(status: EpicBranchStatus<'_>, width: u16) -> String {
    let suffix = format!(" ↑{} ↓{}", status.ahead, status.behind);
    let prefix = "BRANCH: ";
    let branch_budget = (width as usize)
        .saturating_sub(prefix.len() + suffix.chars().count())
        .max(4);
    let branch = truncate_to_width(status.branch, branch_budget as u16, 0);
    format!("{prefix}{branch}{suffix}")
}

fn render_unfocused_epic_placeholder(frame: &mut Frame, area: Rect, theme: &ActiveTheme) {
    let styles = &theme.styles;
    let line = Line::from(Span::styled(
        "No focused epic - supervisor: coordination action=focus_epic id=<epic>",
        styles.text_muted,
    ));
    frame.render_widget(Paragraph::new(line), area);
}

/// Max rows the unfocused overview will list before collapsing the rest
/// into a "+K more" summary line.
const MAX_UNFOCUSED_EPIC_ROWS: usize = 4;

/// Live, session-visible epics for the unfocused overview.
///
/// Reuses `data.tasks_by_epic()` (the same primitive backing the TASKS
/// panel's own unfocused hint, cas-6945) filtered through the shared
/// `epic_is_renderable_source_blind` gate (also cas-582d's own privacy
/// check for the focused-epic path above, and — cas-6185c — the TASKS
/// panel's unfocused hint too) so a session never surfaces a cross-project
/// epic's id/title/counts it has no assignee-based claim to.
///
/// cas-6185c AC2: callers must compute this ONCE per frame and thread the
/// result into both the sizing (`unfocused_epic_progress_height`) and
/// render (`render_unfocused_overview`) steps — it used to be called
/// independently by both, rebuilding the same clone-heavy grouping twice
/// per frame (the exact pattern cas-eb7f deduped next door in the TASKS
/// panel).
fn unfocused_live_epic_groups(data: &DirectorData) -> Vec<cas_factory::EpicGroup> {
    data.live_epic_groups()
        .into_iter()
        .filter(|group| {
            crate::ui::factory::director::tasks::epic_is_renderable_source_blind(
                data,
                &group.epic.id,
            )
        })
        .collect()
}

/// Single source of truth for how many epic rows to show and whether a
/// "+K more" overflow line is needed, given how many live epics exist and
/// how many content rows are available for (epic rows + optional overflow
/// line) — the header and pin-hint footer are reserved separately by every
/// caller and never counted here.
///
/// cas-6185c AC4: used by BOTH `unfocused_epic_progress_height` (deciding
/// how much height to REQUEST, with a generous "space is not the
/// constraint" budget) and `render_unfocused_overview` (rendering within
/// whatever height it actually GOT), so the two can never disagree at the
/// boundary.
///
/// cas-6185c AC3: because the header+hint are reserved by the caller
/// *before* this is consulted, the pin hint can never be starved out by
/// overflow-line accounting the way it used to be (a >4-epic overview
/// could silently drop the hint entirely).
fn plan_unfocused_epic_rows(live_epic_count: usize, available_rows: usize) -> (usize, bool) {
    if live_epic_count == 0 || available_rows == 0 {
        // No room for any row (or nothing to show) — the header's own
        // "Live epics (N):" count is the only signal in this degenerate
        // case, which is still strictly better than the pre-fix bug where
        // even that could be silently blank.
        return (0, false);
    }

    // Never show more than the display cap, regardless of available space
    // — this alone can require an overflow line even when there's plenty
    // of room.
    let capped = live_epic_count.min(MAX_UNFOCUSED_EPIC_ROWS);
    if capped == live_epic_count && capped <= available_rows {
        return (capped, false);
    }

    // Either the display cap or the available space is cutting rows —
    // either way an overflow line is needed, which itself consumes one row
    // from the budget.
    let rows_shown = capped.min(available_rows.saturating_sub(1));
    (rows_shown, true)
}

/// Height needed for the unfocused epic-progress chunk: a header line, one
/// row per live epic (capped), and the focus_epic pin-hint footer. Falls
/// back to the original 2-line placeholder height when there is nothing to
/// show. Never claims more than half the panel so the worker list — the
/// other primary orientation source — always keeps room.
///
/// Takes the already-computed `live_groups` (cas-6185c AC2 — callers build
/// this once per frame via `unfocused_live_epic_groups` and pass it to both
/// this and `render_unfocused_overview`) rather than `data` directly.
fn unfocused_epic_progress_height(live_groups: &[cas_factory::EpicGroup], panel_height: u16) -> u16 {
    if live_groups.is_empty() {
        return 2;
    }

    // Sizing budget is deliberately generous (cap + 1, enough to hold the
    // capped rows AND a trailing overflow line) — at this point we're
    // deciding how much height to REQUEST, not working within a fixed
    // area, so the display cap (not available space) is the only real
    // constraint. `render_unfocused_overview` re-derives the actual plan
    // against whatever height it's actually given.
    let (rows, needs_overflow) =
        plan_unfocused_epic_rows(live_groups.len(), MAX_UNFOCUSED_EPIC_ROWS + 1);
    let desired = 2 + rows + usize::from(needs_overflow); // header + hint + rows [+ overflow]
    (desired as u16).min(panel_height / 2).max(2)
}

/// Render the unfocused epic-progress chunk as a live-epics overview
/// instead of a bare placeholder: a header, one row per live epic (id,
/// truncated title, active/queued subtask counts), a "+K more" line if the
/// list is longer than fits, and the focus_epic pin hint as a one-line
/// footer — the hint always renders (cas-6185c AC3), even when overflow
/// does. Falls back to the original single-line placeholder when there are
/// no session-visible live epics to show.
///
/// Takes the already-computed `live_groups` (cas-6185c AC2, see
/// `unfocused_epic_progress_height`) rather than `data` directly.
fn render_unfocused_overview(
    frame: &mut Frame,
    area: Rect,
    live_groups: &[cas_factory::EpicGroup],
    theme: &ActiveTheme,
) {
    if area.height == 0 || area.width < 10 {
        return;
    }

    if live_groups.is_empty() {
        render_unfocused_epic_placeholder(frame, area, theme);
        return;
    }

    let styles = &theme.styles;
    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(Span::styled(
        format!("Live epics ({}):", live_groups.len()),
        styles.text_info.add_modifier(Modifier::BOLD),
    )));

    // Header + hint are always reserved; plan_unfocused_epic_rows decides
    // rows/overflow within whatever's left (cas-6185c AC3/AC4).
    let available_rows = (area.height as usize).saturating_sub(2);
    let (shown_count, needs_overflow) =
        plan_unfocused_epic_rows(live_groups.len(), available_rows);

    for group in live_groups.iter().take(shown_count) {
        let active = group
            .subtasks
            .iter()
            .filter(|t| t.status == TaskStatus::InProgress)
            .count();
        let queued = group.subtasks.iter().filter(|task| task.status == TaskStatus::Open).count();
        let counts = format!("  {active} active, {queued} queued");
        let title_budget = (area.width as usize)
            .saturating_sub(group.epic.id.len() + 4 + counts.len())
            .max(4) as u16;
        let title = truncate_to_width(&group.epic.title, title_budget, 0);

        lines.push(Line::from(vec![
            Span::styled(format!("  {} ", group.epic.id), styles.text_info),
            Span::styled(title, styles.text_primary),
            Span::styled(counts, styles.text_muted),
        ]));
    }

    if needs_overflow {
        let remaining = live_groups.len() - shown_count;
        lines.push(Line::from(Span::styled(
            format!("  … +{remaining} more"),
            styles.text_muted.add_modifier(Modifier::ITALIC),
        )));
    }

    // The pin hint is unconditional — never gated on remaining space. If
    // the panel is too short even for header+hint, the caller's height
    // function guarantees at least 2 rows, so this always fits.
    lines.push(Line::from(Span::styled(
        "Pin: coordination action=focus_epic id=<epic>",
        styles.text_muted,
    )));

    frame.render_widget(Paragraph::new(lines), area);
}

/// Render worker list with current tasks
fn render_worker_list(
    frame: &mut Frame,
    area: Rect,
    data: &DirectorData,
    theme: &ActiveTheme,
    selected: Option<usize>,
    supervisor_name: &str,
) {
    if area.height == 0 || area.width < 10 {
        return;
    }

    let styles = &theme.styles;
    let palette = &theme.palette;

    let mut lines: Vec<Line> = Vec::new();
    let max_lines = area.height as usize;

    for (idx, agent) in data.agents.iter().enumerate() {
        if lines.len() >= max_lines {
            break;
        }

        // Status indicator
        let (status_char, status_color) =
            agent_helpers::agent_status_icon_simple(agent, palette, theme.is_minions());

        let is_selected = selected == Some(idx);
        let name_style = if is_selected {
            styles.text_info.add_modifier(Modifier::BOLD)
        } else {
            styles.text_primary
        };

        // Build task info
        let current_task = agent_helpers::find_agent_in_progress_task(agent, data);
        let task_info = if let Some(task) = current_task {
            format!("▸ {} {}", task.id, task.title)
        } else if let Some((activity, _)) = &agent.latest_activity {
            activity.clone()
        } else {
            "idle".to_string()
        };

        // Calculate available width for task info
        // Format: "[●] name: task_info"
        let prefix_len = 4 + agent.name.len() + 2; // "[●] " + name + ": "
        let task_display = truncate_to_width(&task_info, area.width, prefix_len);

        let line = Line::from(vec![
            Span::raw("["),
            Span::styled(status_char, Style::default().fg(status_color)),
            Span::raw("] "),
            Span::styled(&agent.name, name_style),
            Span::styled(": ", styles.text_muted),
            Span::styled(
                task_display,
                if current_task.is_some() {
                    styles.text_primary
                } else {
                    styles.text_muted
                },
            ),
        ]);

        lines.push(line);
    }

    // Show missing supervisor explicitly so factory state doesn't silently hide it.
    let supervisor_missing = !supervisor_name.is_empty()
        && !data
            .agents
            .iter()
            .any(|agent| agent.name == supervisor_name);
    if supervisor_missing && lines.len() < max_lines {
        let line = Line::from(vec![
            Span::raw("["),
            Span::styled("⊘", Style::default().fg(palette.agent_dead)),
            Span::raw("] "),
            Span::styled(
                supervisor_name,
                styles.text_info.add_modifier(Modifier::BOLD),
            ),
            Span::styled(": ", styles.text_muted),
            Span::styled("not registered", styles.text_muted),
        ]);
        lines.push(line);
    }

    // Fill remaining space with empty lines or "no workers" message
    if lines.is_empty() {
        lines.push(Line::from(Span::styled(
            "No agents registered yet",
            styles.text_muted,
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

/// Render summary bar with counts
fn render_summary_bar(
    frame: &mut Frame,
    area: Rect,
    data: &DirectorData,
    theme: &ActiveTheme,
    supervisor_name: &str,
) {
    if area.height == 0 || area.width < 10 {
        return;
    }

    let styles = &theme.styles;
    let palette = &theme.palette;

    // Count agent statuses
    let status_counts = agent_helpers::count_agent_statuses(&data.agents);
    let active = status_counts.active;
    let idle = status_counts.idle;
    let mut dead = status_counts.dead;

    // Include missing supervisor as dead
    if !supervisor_name.is_empty()
        && !data
            .agents
            .iter()
            .any(|agent| agent.name == supervisor_name)
    {
        dead += 1;
    }

    let queue_count = data
        .ready_tasks
        .iter()
        .filter(|task| data.is_live_queue_task(task))
        .count();
    let blocked_count = data
        .in_progress_tasks
        .iter()
        .chain(data.ready_tasks.iter())
        .filter(|t| t.status == TaskStatus::Blocked)
        .count();
    let verification_debt = data
        .activity
        .iter()
        .filter(|e| e.event_type == EventType::WorkerVerificationBlocked)
        .map(|e| e.entity_id.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .len();

    let mut spans = vec![
        Span::styled("Active ", styles.text_muted),
        Span::styled(
            active.to_string(),
            Style::default().fg(palette.agent_active),
        ),
        Span::raw(" │ "),
        Span::styled("Idle ", styles.text_muted),
        Span::styled(idle.to_string(), Style::default().fg(palette.agent_idle)),
        Span::raw(" │ "),
        Span::styled("Queue ", styles.text_muted),
        Span::styled(
            queue_count.to_string(),
            Style::default().fg(palette.status_info),
        ),
    ];

    // Show blocked count if any
    if blocked_count > 0 {
        spans.extend([
            Span::raw(" │ "),
            Span::styled("Blocked ", styles.text_muted),
            Span::styled(
                blocked_count.to_string(),
                Style::default().fg(palette.status_warning),
            ),
        ]);
    }

    if verification_debt > 0 {
        spans.extend([
            Span::raw(" │ "),
            Span::styled("VerifDebt ", styles.text_muted),
            Span::styled(
                verification_debt.to_string(),
                Style::default().fg(palette.status_warning),
            ),
        ]);
    }

    // Show errors if any
    if dead > 0 {
        spans.extend([
            Span::raw(" │ "),
            Span::styled("Errors ", styles.text_muted),
            Span::styled(dead.to_string(), Style::default().fg(palette.status_error)),
        ]);
    }

    let line = Line::from(spans);
    frame.render_widget(Paragraph::new(line), area);
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use cas_factory::{AgentSummary, DirectorData, TaskSummary};
    use cas_types::{AgentStatus, Priority, TaskStatus, TaskType};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    use crate::ui::theme::ActiveTheme;

    use super::render_with_focus;

    fn task(id: &str, title: &str, status: TaskStatus, task_type: TaskType) -> TaskSummary {
        TaskSummary {
            id: id.to_string(),
            title: title.to_string(),
            status,
            priority: Priority::MEDIUM,
            assignee: None,
            task_type,
            epic: None,
            branch: None,
            updated_at: None,
        epic_verification_owner: None,
        }
        }

    fn data_with_unrelated_epic() -> DirectorData {
        DirectorData {
            ready_tasks: vec![TaskSummary {
                id: "cas-foreign-child".to_string(),
                title: "Foreign child".to_string(),
                status: TaskStatus::Open,
                priority: Priority::MEDIUM,
                assignee: Some("other-agent".to_string()),
                task_type: TaskType::Task,
                epic: Some("cas-foreign".to_string()),
                branch: None,
                updated_at: None,
            epic_verification_owner: None,
        }],
            in_progress_tasks: Vec::new(),
            epic_tasks: vec![task(
                "cas-foreign",
                "Foreign in-progress epic",
                TaskStatus::InProgress,
                TaskType::Epic,
            )],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::from([(
                "session-agent".to_string(),
                "worker-one".to_string(),
            )]),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        }
    }

    fn agent(id: &str, name: &str) -> AgentSummary {
        AgentSummary {
            id: id.to_string(),
            name: name.to_string(),
            status: AgentStatus::Active,
            registered_at: chrono::Utc::now(),
            current_task: None,
            latest_activity: None,
            last_heartbeat: Some(chrono::Utc::now()),
            pending_messages: 0,
            pending_supervisor_messages: 0,
            latest_supervisor_message_at: None,
            active_lease: None,
            effort: None,
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
    fn factory_radar_renders_unfocused_placeholder_instead_of_foreign_epic() {
        let data = data_with_unrelated_epic();
        let backend = TestBackend::new(90, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();

        terminal
            .draw(|frame| {
                render_with_focus(
                    frame,
                    frame.area(),
                    &data,
                    &theme,
                    Some("cas-foreign"),
                    None,
                    false,
                    None,
                    "supervisor",
                    false,
                );
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("No focused epic"));
        assert!(text.contains("coordination action=focus_epic id=<epic>"));
        assert!(!text.contains("EPIC: cas-foreign"));
    }

    #[test]
    fn factory_radar_renders_session_owned_focused_epic_source_blind() {
        let mut data = data_with_unrelated_epic();
        data.ready_tasks[0].assignee = Some("session-agent".to_string());
        let backend = TestBackend::new(90, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();

        terminal
            .draw(|frame| {
                render_with_focus(
                    frame,
                    frame.area(),
                    &data,
                    &theme,
                    Some("cas-foreign"),
                    None,
                    false,
                    None,
                    "supervisor",
                    false,
                );
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("EPIC: cas-foreign"));
        assert!(!text.contains("No focused epic"));
    }

    #[test]
    fn factory_radar_renders_unassigned_focused_epic_source_blind() {
        let mut data = data_with_unrelated_epic();
        data.ready_tasks[0].assignee = None;
        let backend = TestBackend::new(90, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();

        terminal
            .draw(|frame| {
                render_with_focus(
                    frame,
                    frame.area(),
                    &data,
                    &theme,
                    Some("cas-foreign"),
                    None,
                    false,
                    None,
                    "supervisor",
                    false,
                );
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("EPIC: cas-foreign"));
        assert!(!text.contains("No focused epic"));
    }

    #[test]
    fn factory_radar_renders_epic_branch_ahead_behind() {
        let mut data = data_with_unrelated_epic();
        data.ready_tasks[0].assignee = Some("session-agent".to_string());
        let backend = TestBackend::new(110, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();

        terminal
            .draw(|frame| {
                render_with_focus(
                    frame,
                    frame.area(),
                    &data,
                    &theme,
                    Some("cas-foreign"),
                    Some(crate::ui::factory::director::EpicBranchStatus {
                        branch: "epic/epic-factory-tui-visual-information-overhaul-osc-8-cas-ebc1",
                        ahead: 3,
                        behind: 1,
                    }),
                    false,
                    None,
                    "supervisor",
                    false,
                );
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("EPIC: cas-foreign"));
        assert!(text.contains("BRANCH: epic/epic-factory-tui-visual-information-overhaul"));
        assert!(text.contains("↑3 ↓1"));
    }

    #[test]
    fn factory_radar_worker_rows_show_task_chips_for_id_and_display_name_assignees() {
        let mut data = data_with_unrelated_epic();
        data.agents = vec![
            agent("agent-id-1", "worker-one"),
            agent("agent-id-2", "worker-two"),
            agent("agent-id-3", "worker-three"),
        ];
        data.in_progress_tasks = vec![
            TaskSummary {
                id: "cas-id1".to_string(),
                title: "Assigned by agent id".to_string(),
                status: TaskStatus::InProgress,
                priority: Priority::MEDIUM,
                assignee: Some("agent-id-1".to_string()),
                task_type: TaskType::Task,
                epic: Some("cas-foreign".to_string()),
                branch: None,
                updated_at: None,
            epic_verification_owner: None,
        },
            TaskSummary {
                id: "cas-name2".to_string(),
                title: "Assigned by display name".to_string(),
                status: TaskStatus::InProgress,
                priority: Priority::MEDIUM,
                assignee: Some("worker-two".to_string()),
                task_type: TaskType::Task,
                epic: Some("cas-foreign".to_string()),
                branch: None,
                updated_at: None,
            epic_verification_owner: None,
        },
        ];
        let backend = TestBackend::new(120, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();

        terminal
            .draw(|frame| {
                render_with_focus(
                    frame,
                    frame.area(),
                    &data,
                    &theme,
                    Some("cas-foreign"),
                    None,
                    false,
                    None,
                    "supervisor",
                    false,
                );
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("worker-one: ▸ cas-id1 Assigned by agent id"));
        assert!(text.contains("worker-two: ▸ cas-name2 Assigned by display name"));
        assert!(text.contains("worker-three: idle"));
    }

    fn subtask(
        id: &str,
        title: &str,
        status: TaskStatus,
        epic: &str,
        assignee: Option<&str>,
    ) -> TaskSummary {
        TaskSummary {
            id: id.to_string(),
            title: title.to_string(),
            status,
            priority: Priority::MEDIUM,
            assignee: assignee.map(str::to_string),
            task_type: TaskType::Task,
            epic: Some(epic.to_string()),
            branch: None,
            updated_at: None,
        epic_verification_owner: None,
        }
        }

    fn data_with_two_live_epics() -> DirectorData {
        DirectorData {
            ready_tasks: vec![
                subtask("cas-a1", "Alpha queued", TaskStatus::Open, "cas-alpha", None),
                subtask(
                    "cas-b1",
                    "Beta queued",
                    TaskStatus::Open,
                    "cas-beta",
                    Some("worker-one"),
                ),
            ],
            in_progress_tasks: vec![subtask(
                "cas-a2",
                "Alpha active",
                TaskStatus::InProgress,
                "cas-alpha",
                None,
            )],
            epic_tasks: vec![
                task("cas-alpha", "Alpha Epic", TaskStatus::Open, TaskType::Epic),
                task("cas-beta", "Beta Epic", TaskStatus::Open, TaskType::Epic),
            ],
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::from([(
                "worker-one".to_string(),
                "worker-one".to_string(),
            )]),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        }
    }

    /// cas-582d AC1/AC3: unfocused with multiple live epics must list them
    /// (not the bare "No focused epic" dead-zone), with basic progress
    /// counts and the pin hint demoted to a single footer line.
    #[test]
    fn factory_radar_unfocused_overview_lists_multiple_live_epics() {
        let data = data_with_two_live_epics();
        let backend = TestBackend::new(90, 14);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();

        terminal
            .draw(|frame| {
                render_with_focus(
                    frame,
                    frame.area(),
                    &data,
                    &theme,
                    None,
                    None,
                    false,
                    None,
                    "supervisor",
                    false,
                );
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("Live epics (2):"),
            "should show a live-epics overview header: {text}"
        );
        assert!(text.contains("cas-alpha"), "missing alpha epic id: {text}");
        assert!(
            text.contains("1 active, 1 queued"),
            "alpha epic counts wrong: {text}"
        );
        assert!(text.contains("cas-beta"), "missing beta epic id: {text}");
        assert!(
            text.contains("0 active, 1 queued"),
            "beta epic counts wrong: {text}"
        );
        assert!(
            text.contains("Pin: coordination action=focus_epic id=<epic>"),
            "pin hint should still appear as a footer line: {text}"
        );
        assert!(
            !text.contains("No focused epic"),
            "bare dead-zone placeholder must not render when live epics exist: {text}"
        );
    }

    #[test]
    fn factory_radar_ignores_closed_epic_residue_and_counts_only_live_queue_tasks() {
        let mut data = data_with_two_live_epics();
        data.ready_tasks.push(subtask("cas-closed-child", "Stale queued child", TaskStatus::Open, "cas-closed", None));
        data.epic_tasks.push(task("cas-closed", "Closed epic with residual queue row", TaskStatus::Closed, TaskType::Epic));
        data.epic_tasks.push(task("cas-empty", "Open epic without children", TaskStatus::Open, TaskType::Epic));

        let backend = TestBackend::new(100, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();
        terminal.draw(|frame| {
            render_with_focus(frame, frame.area(), &data, &theme, None, None, false, None, "supervisor", false);
        }).unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("Live epics (3):"), "wrong liveness set: {text}");
        assert!(text.contains("cas-empty"), "open empty epic must render: {text}");
        assert!(!text.contains("cas-closed"), "closed epic and its residual child must not render: {text}");
        assert!(text.contains("Queue 2"), "terminal residue inflated queue: {text}");
    }

    /// cas-582d AC3: unfocused with no session-visible live epics still
    /// falls back to the original single-line placeholder — including the
    /// case where the only epic present is foreign (source-blind gate),
    /// proving the overview never leaks a cross-project epic id/title.
    #[test]
    fn factory_radar_unfocused_overview_falls_back_when_no_live_epics() {
        let data = data_with_unrelated_epic();
        let backend = TestBackend::new(90, 12);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();

        terminal
            .draw(|frame| {
                render_with_focus(
                    frame,
                    frame.area(),
                    &data,
                    &theme,
                    None,
                    None,
                    false,
                    None,
                    "supervisor",
                    false,
                );
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(text.contains("No focused epic"));
        assert!(text.contains("coordination action=focus_epic id=<epic>"));
        assert!(
            !text.contains("Live epics"),
            "must not claim a live-epics overview when nothing is session-visible: {text}"
        );
        assert!(
            !text.contains("cas-foreign"),
            "foreign epic id must not leak into the unfocused overview: {text}"
        );
    }

    /// cas-582d AC3: the worker-with-task row must still render correctly
    /// alongside the new live-epics overview when unfocused — proving the
    /// dynamic epic-progress sizing doesn't starve or corrupt the worker
    /// list.
    #[test]
    fn factory_radar_unfocused_overview_worker_row_shows_current_task() {
        let mut data = data_with_two_live_epics();
        data.agents = vec![agent("agent-1", "worker-one")];
        // `find_agent_in_progress_task` only matches `in_progress_tasks`
        // (ready/queued tasks don't count as "current"), so assign the
        // in-progress alpha subtask to the worker.
        data.in_progress_tasks[0].assignee = Some("worker-one".to_string());
        let backend = TestBackend::new(90, 16);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();

        terminal
            .draw(|frame| {
                render_with_focus(
                    frame,
                    frame.area(),
                    &data,
                    &theme,
                    None,
                    None,
                    false,
                    None,
                    "supervisor",
                    false,
                );
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("Live epics (2):"),
            "overview should still render: {text}"
        );
        assert!(
            text.contains("worker-one: ▸ cas-a2 Alpha active"),
            "worker row with current task should still render unfocused: {text}"
        );
    }

    // --- cas-6185c: plan_unfocused_epic_rows / sizing-render agreement ---

    /// cas-6185c AC4: exhaustive coverage of the shared row-planning
    /// predicate, including the exact boundary that used to make sizing
    /// and rendering disagree (a live-epic count over the display cap).
    #[test]
    fn plan_unfocused_epic_rows_covers_boundaries() {
        // Nothing to show.
        assert_eq!(super::plan_unfocused_epic_rows(0, 10), (0, false));

        // Fits comfortably under both the cap and the space budget.
        assert_eq!(super::plan_unfocused_epic_rows(3, 5), (3, false));

        // Under the cap, but space-constrained: 3 epics, only 2 rows of
        // space -> 1 shown + overflow line (reserves 1 row for it).
        assert_eq!(super::plan_unfocused_epic_rows(3, 2), (1, true));

        // Over the display cap (4) but plenty of space: still capped at 4,
        // and now needs the overflow line too (this is the exact bug: the
        // OLD sizing function budgeted only `2 + capped_rows`, never the
        // +1 for the overflow line, so render_unfocused_overview always
        // ran one line short and dropped the pin hint).
        assert_eq!(super::plan_unfocused_epic_rows(6, 100), (4, true));

        // Degenerate: no room for even one row — the header's own count
        // is the only signal left, no overflow line either.
        assert_eq!(super::plan_unfocused_epic_rows(5, 0), (0, false));

        // Exactly enough room for the cap and nothing else: cap not
        // exceeded, so still no overflow needed.
        assert_eq!(super::plan_unfocused_epic_rows(4, 4), (4, false));
    }

    fn data_with_six_live_epics() -> DirectorData {
        let ids = ["cas-a", "cas-b", "cas-c", "cas-d", "cas-e", "cas-f"];
        let ready_tasks = ids
            .iter()
            .map(|id| subtask(&format!("{id}-1"), "Queued", TaskStatus::Open, id, None))
            .collect();
        let epic_tasks = ids
            .iter()
            .map(|id| task(id, "Epic", TaskStatus::Open, TaskType::Epic))
            .collect();
        DirectorData {
            ready_tasks,
            in_progress_tasks: Vec::new(),
            epic_tasks,
            agents: Vec::new(),
            activity: Vec::new(),
            agent_id_to_name: HashMap::new(),
            changes: Vec::new(),
            git_loaded: false,
            reminders: Vec::new(),
            epic_closed_counts: HashMap::new(),
        }
    }

    /// cas-6185c AC3: the pin hint must always be visible even when there
    /// are more live epics than the display cap (>4) — the bug this task
    /// exists to fix silently dropped the hint in exactly this case,
    /// because the old sizing function never budgeted a line for the
    /// overflow indicator it then unconditionally rendered.
    #[test]
    fn factory_radar_unfocused_overview_keeps_pin_hint_with_overflow() {
        let data = data_with_six_live_epics();
        let backend = TestBackend::new(90, 20);
        let mut terminal = Terminal::new(backend).unwrap();
        let theme = ActiveTheme::default();

        terminal
            .draw(|frame| {
                render_with_focus(
                    frame,
                    frame.area(),
                    &data,
                    &theme,
                    None,
                    None,
                    false,
                    None,
                    "supervisor",
                    false,
                );
            })
            .unwrap();

        let text = buffer_text(&terminal);
        assert!(
            text.contains("Live epics (6):"),
            "header should report the true total: {text}"
        );
        assert!(
            text.contains("more"),
            "overflow ('+K more') line should render when over the display cap: {text}"
        );
        assert!(
            text.contains("Pin: coordination action=focus_epic id=<epic>"),
            "pin hint must survive the overflow case, not be silently dropped: {text}"
        );
    }
}
