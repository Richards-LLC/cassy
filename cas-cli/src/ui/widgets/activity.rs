//! Activity log widget for sidecar and factory TUI

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem, ListState};
use ratatui::Frame;

use cas_types::{Event, EventType};

use crate::ui::theme::{get_agent_color, ActiveTheme};

use crate::ui::widgets::utils::{event_type_color, format_relative, format_relative_at, truncate};

const COMPACT_WRAP_RECENT_COUNT: usize = 5;

/// Configuration for activity list rendering
#[derive(Debug, Default)]
pub struct ActivityConfig {
    /// Map from agent ID to agent name (for color lookup)
    pub agent_id_to_name: HashMap<String, String>,
    /// IDs of currently active agents
    pub active_agent_ids: HashSet<String>,
    /// Maximum number of events to show
    pub max_events: usize,
}

impl ActivityConfig {
    pub fn new() -> Self {
        Self {
            max_events: 20,
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
            max_events: 20,
        }
    }
}

/// Render a stateless activity list (for factory director)
pub fn render_activity_list(
    frame: &mut Frame,
    area: Rect,
    events: &[Event],
    theme: &ActiveTheme,
    config: &ActivityConfig,
    title: Option<&str>,
) {
    let palette = &theme.palette;
    let block = if let Some(title) = title {
        Block::default()
            .title(format!(" {} ({}) ", title, events.len()))
            .borders(Borders::TOP)
            .border_style(Style::default().fg(palette.border_muted))
    } else {
        Block::default()
    };

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let items = build_activity_items(events, theme, config, inner.width, inner.height as usize);
    let list = List::new(items);
    frame.render_widget(list, inner);
}

/// Render a stateful activity list (for sidecar with selection)
pub fn render_activity_list_with_state(
    frame: &mut Frame,
    area: Rect,
    events: &[Event],
    theme: &ActiveTheme,
    config: &ActivityConfig,
    state: &mut ListState,
    block: Block,
) {
    let styles = &theme.styles;
    let inner = block.inner(area);
    let items = build_activity_items(events, theme, config, inner.width, config.max_events);
    let list = List::new(items)
        .block(block)
        .highlight_style(styles.bg_selection);
    frame.render_stateful_widget(list, area, state);
}

/// Build activity list items
fn build_activity_items(
    events: &[Event],
    theme: &ActiveTheme,
    config: &ActivityConfig,
    width: u16,
    max_items: usize,
) -> Vec<ListItem<'static>> {
    let palette = &theme.palette;
    let styles = &theme.styles;
    let items: Vec<ListItem> = events
        .iter()
        .take(max_items)
        .map(|event| {
            let time = format_relative(event.created_at);

            // Color by agent if present and active, otherwise by event type
            let event_color = event
                .session_id
                .as_ref()
                .filter(|id| config.active_agent_ids.contains(*id))
                .and_then(|id| config.agent_id_to_name.get(id))
                .map(|name| get_agent_color(name))
                .unwrap_or_else(|| event_type_color(&event.event_type, palette));

            let summary = truncate(&event.summary, width.saturating_sub(8) as usize);

            ListItem::new(Line::from(vec![
                Span::styled(format!("{time:>4} "), styles.text_muted),
                Span::styled(
                    format!("{} ", event.icon()),
                    Style::default().fg(event_color),
                ),
                Span::styled(summary, Style::default().fg(event_color)),
            ]))
        })
        .collect();

    if items.is_empty() {
        vec![ListItem::new(Line::from(vec![Span::styled(
            "No activity",
            styles.text_muted.add_modifier(Modifier::ITALIC),
        )]))]
    } else {
        items
    }
}

/// Render compact activity list for factory director
pub fn render_compact_activity_list(
    frame: &mut Frame,
    area: Rect,
    events: &[Event],
    agent_id_to_name: &HashMap<String, String>,
    theme: &ActiveTheme,
    focused: bool,
    backend_tags: &HashMap<String, &'static str>,
) {
    let styles = &theme.styles;
    let border_style = if focused {
        styles.border_focused
    } else {
        styles.border_default
    };

    let focus_marker = if focused { "▶" } else { " " };
    let block = Block::default()
        .title(format!(" {} ACTIVITY ({}) ", focus_marker, events.len()))
        .borders(Borders::ALL)
        .border_type(BorderType::Plain)
        .border_style(border_style);

    let inner = block.inner(area);
    frame.render_widget(block, area);

    let items = build_compact_activity_items_at(
        events,
        agent_id_to_name,
        theme,
        inner.width,
        inner.height as usize,
        Utc::now(),
        backend_tags,
    );

    if items.is_empty() {
        let empty_list = List::new(vec![ListItem::new(Line::from(vec![Span::styled(
            "No activity",
            styles.text_muted.add_modifier(Modifier::ITALIC),
        )]))]);
        frame.render_widget(empty_list, inner);
    } else {
        let list = List::new(items);
        frame.render_widget(list, inner);
    }
}

fn build_compact_activity_items_at(
    events: &[Event],
    agent_id_to_name: &HashMap<String, String>,
    theme: &ActiveTheme,
    width: u16,
    max_items: usize,
    now: DateTime<Utc>,
    backend_tags: &HashMap<String, &'static str>,
) -> Vec<ListItem<'static>> {
    let styles = &theme.styles;
    let items: Vec<ListItem> = events
        .iter()
        .take(max_items)
        .enumerate()
        .map(|(idx, event)| {
            compact_activity_item(
                event,
                agent_id_to_name,
                theme,
                width,
                now,
                idx < COMPACT_WRAP_RECENT_COUNT,
                backend_tags,
            )
        })
        .collect();

    if items.is_empty() {
        vec![ListItem::new(Line::from(vec![Span::styled(
            "No activity",
            styles.text_muted.add_modifier(Modifier::ITALIC),
        )]))]
    } else {
        items
    }
}

fn compact_activity_item(
    event: &Event,
    agent_id_to_name: &HashMap<String, String>,
    theme: &ActiveTheme,
    width: u16,
    now: DateTime<Utc>,
    wrap_summary: bool,
    backend_tags: &HashMap<String, &'static str>,
) -> ListItem<'static> {
    let styles = &theme.styles;
    let time = format_relative_at(event.created_at, now);
    let icon = event.icon();
    let icon_style = event_category_style(&event.event_type, styles);
    // Summary keeps per-agent attribution color when the event's session
    // resolves to a known agent; the icon carries the event-category style.
    let summary_style = event
        .session_id
        .as_ref()
        .and_then(|id| agent_id_to_name.get(id))
        .map(|name| Style::default().fg(get_agent_color(name)))
        .unwrap_or_else(|| event_category_style(&event.event_type, styles));
    let prefix = format!("{time:>4} {icon} ");
    let prefix_width = prefix.chars().count();
    let summary_width = (width as usize).saturating_sub(prefix_width).max(1);
    let activity_summary = format_activity_summary(event, backend_tags);

    if !wrap_summary {
        let summary = truncate(&activity_summary, summary_width);
        return ListItem::new(Line::from(vec![
            Span::styled(format!("{time:>4} "), styles.text_muted),
            Span::styled(format!("{icon} "), icon_style),
            Span::styled(summary, summary_style),
        ]));
    }

    let wrapped = wrap_text(&activity_summary, summary_width);
    let mut lines = Vec::with_capacity(wrapped.len().max(1));
    let first = wrapped.first().cloned().unwrap_or_default();
    lines.push(Line::from(vec![
        Span::styled(format!("{time:>4} "), styles.text_muted),
        Span::styled(format!("{icon} "), icon_style),
        Span::styled(first, summary_style),
    ]));

    for continuation in wrapped.into_iter().skip(1) {
        lines.push(Line::from(vec![
            Span::raw(" ".repeat(prefix_width)),
            Span::styled(continuation, summary_style),
        ]));
    }

    ListItem::new(lines)
}

/// Prefix verification decisions with the backend of the agent that recorded
/// them. Other activity rows retain their established text exactly.
pub fn format_activity_summary(
    event: &Event,
    backend_tags: &HashMap<String, &'static str>,
) -> String {
    if event.event_type == EventType::VerificationAdded {
        if let Some(backend) = event
            .session_id
            .as_ref()
            .and_then(|agent_id| backend_tags.get(agent_id))
        {
            return format!("[{backend}] {}", event.summary);
        }
    }
    event.summary.clone()
}

fn event_category_style(event_type: &EventType, styles: &crate::ui::theme::Styles) -> Style {
    match event_type {
        EventType::AgentRegistered
        | EventType::AgentHeartbeat
        | EventType::AgentShutdown
        | EventType::FactoryStarted
        | EventType::FactoryStopped
        | EventType::WorkerAssigned
        | EventType::SupervisorNotified
        | EventType::SupervisorInjected
        | EventType::WorkerSubagentSpawned
        | EventType::MemoryStored
        | EventType::SkillUsed
        | EventType::RulePromoted => styles.text_info,
        EventType::TaskCreated
        | EventType::TaskStarted
        | EventType::TaskCompleted
        | EventType::TaskNoteAdded
        | EventType::WorkerCompleted
        | EventType::WorkerSubagentCompleted
        | EventType::EpicSubtasksComplete => styles.text_success,
        EventType::TaskBlocked
        | EventType::WorkerVerificationBlocked
        | EventType::VerificationStarted
        | EventType::VerificationAdded => styles.text_warning,
        EventType::WorkerDied | EventType::TaskDeleted | EventType::AuditTrailGap => {
            styles.text_error
        }
        EventType::WorkerFileEdited | EventType::WorkerGitCommit => styles.text_accent,
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return vec![String::new()];
    }

    let width = width.max(1);
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let word_len = word.chars().count();
        if current.is_empty() {
            push_wrapped_word(word, width, &mut current, &mut lines);
        } else if current.chars().count() + 1 + word_len <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            push_wrapped_word(word, width, &mut current, &mut lines);
        }
    }

    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn push_wrapped_word(word: &str, width: usize, current: &mut String, lines: &mut Vec<String>) {
    for ch in word.chars() {
        if current.chars().count() >= width {
            lines.push(std::mem::take(current));
        }
        current.push(ch);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use chrono::{Duration, TimeZone};
    use ratatui::backend::TestBackend;
    use ratatui::Terminal;

    use cas_types::EventEntityType;

    fn test_event(event_type: EventType, summary: &str, created_at: DateTime<Utc>) -> Event {
        let mut event = Event::new(event_type, EventEntityType::Task, "cas-test", summary);
        event.created_at = created_at;
        event
    }

    fn render_items_text(items: Vec<ListItem<'static>>, width: u16, height: u16) -> String {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                frame.render_widget(List::new(items), frame.area());
            })
            .unwrap();

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
    fn compact_activity_wraps_recent_entries_and_truncates_older_entries() {
        let theme = ActiveTheme::default_dark();
        let now = Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).unwrap();
        let long_summary = "alpha beta gamma delta epsilon zeta eta theta";
        let events: Vec<Event> = (0..6)
            .map(|idx| {
                test_event(
                    EventType::TaskNoteAdded,
                    &format!("{long_summary} {idx}"),
                    now - Duration::minutes(idx),
                )
            })
            .collect();

        let items = build_compact_activity_items_at(
            &events,
            &HashMap::new(),
            &theme,
            26,
            6,
            now,
            &HashMap::new(),
        );

        assert_eq!(items.len(), 6);
        assert!(items[..COMPACT_WRAP_RECENT_COUNT]
            .iter()
            .all(|item| item.height() > 1));
        assert_eq!(items[COMPACT_WRAP_RECENT_COUNT].height(), 1);

        let text = render_items_text(items, 26, 24);
        assert!(text.contains("alpha beta"));
        assert!(text.contains("theta 0"), "{text}");
        assert!(text.contains("..."));
    }

    #[test]
    fn compact_activity_uses_theme_styles_by_event_category() {
        let theme = ActiveTheme::default_dark();
        let styles = &theme.styles;

        assert_eq!(
            event_category_style(&EventType::AgentRegistered, styles),
            styles.text_info
        );
        assert_eq!(
            event_category_style(&EventType::TaskCompleted, styles),
            styles.text_success
        );
        assert_eq!(
            event_category_style(&EventType::VerificationAdded, styles),
            styles.text_warning
        );
        assert_eq!(
            event_category_style(&EventType::WorkerDied, styles),
            styles.text_error
        );
        assert_eq!(
            event_category_style(&EventType::WorkerGitCommit, styles),
            styles.text_accent
        );
    }

    #[test]
    fn compact_activity_relative_timestamps_use_current_render_clock() {
        let theme = ActiveTheme::default_dark();
        let created_at = Utc.with_ymd_and_hms(2026, 7, 6, 12, 0, 0).unwrap();
        let events = vec![test_event(
            EventType::WorkerGitCommit,
            "commit pushed",
            created_at,
        )];

        let first_render = render_items_text(
            build_compact_activity_items_at(
                &events,
                &HashMap::new(),
                &theme,
                40,
                1,
                created_at + Duration::minutes(3),
                &HashMap::new(),
            ),
            40,
            4,
        );
        let second_render = render_items_text(
            build_compact_activity_items_at(
                &events,
                &HashMap::new(),
                &theme,
                40,
                1,
                created_at + Duration::minutes(16),
                &HashMap::new(),
            ),
            40,
            4,
        );

        assert!(first_render.contains("3m"));
        assert!(second_render.contains("16m"));
    }

    #[test]
    fn verification_summary_identifies_the_deciding_backend() {
        let mut event = test_event(
            EventType::VerificationAdded,
            "Verification approved: cas-1234",
            Utc::now(),
        );
        event.session_id = Some("supervisor-session".to_string());
        let backends = HashMap::from([("supervisor-session".to_string(), "codex")]);

        assert_eq!(
            format_activity_summary(&event, &backends),
            "[codex] Verification approved: cas-1234"
        );
    }
}
