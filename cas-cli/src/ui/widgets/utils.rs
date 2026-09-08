//! Utility functions for widget rendering

use chrono::{DateTime, Utc};
use ratatui::style::Color;

use crate::ui::theme::Palette;
use cas_types::Priority;

/// Truncate a string to max length with ellipsis
pub fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else if max_len <= 3 {
        "...".to_string()
    } else {
        format!("{}...", prefix_at_char_boundary(s, max_len - 3))
    }
}

/// Truncate a string to fit within a width, accounting for prefix characters
pub fn truncate_to_width(s: &str, width: u16, prefix_chars: usize) -> String {
    let available = (width as usize).saturating_sub(prefix_chars + 3);
    if s.len() <= available {
        s.to_string()
    } else if available <= 3 {
        "...".to_string()
    } else {
        format!(
            "{}...",
            prefix_at_char_boundary(s, available.saturating_sub(3))
        )
    }
}

fn prefix_at_char_boundary(s: &str, max_bytes: usize) -> &str {
    let mut end = max_bytes.min(s.len());
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Format a datetime as relative time (e.g., "5m", "2h", "3d")
pub fn format_relative(dt: DateTime<Utc>) -> String {
    format_relative_at(dt, Utc::now())
}

/// Format a datetime as relative time against an explicit clock.
pub fn format_relative_at(dt: DateTime<Utc>, now: DateTime<Utc>) -> String {
    let diff = now.signed_duration_since(dt);
    let secs = diff.num_seconds();

    if secs < 0 {
        "now".to_string()
    } else if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86400)
    }
}

/// Get color for a task priority
pub fn priority_color(priority: Priority, palette: &Palette) -> Color {
    match priority.0 {
        0 => palette.priority_critical,
        1 => palette.priority_high,
        2 => palette.priority_medium,
        3 => palette.priority_low,
        _ => palette.priority_backlog,
    }
}

/// Get color for an event type
pub fn event_type_color(event_type: &cas_types::EventType, palette: &Palette) -> Color {
    use cas_types::EventType;
    match event_type {
        EventType::AgentRegistered => palette.status_success,
        EventType::AgentHeartbeat => palette.status_neutral,
        EventType::AgentShutdown => palette.status_error,
        EventType::TaskCreated => palette.status_info,
        EventType::TaskStarted => palette.status_warning,
        EventType::TaskCompleted => palette.status_success,
        EventType::TaskBlocked => palette.status_error,
        EventType::TaskNoteAdded => palette.text_primary,
        EventType::TaskDeleted => palette.status_error,
        EventType::MemoryStored => palette.status_info,
        EventType::RulePromoted => palette.accent,
        EventType::SkillUsed => palette.status_warning,
        EventType::FactoryStarted => palette.status_success,
        EventType::FactoryStopped => palette.status_error,
        EventType::WorkerDied => palette.status_error,
        EventType::WorkerAssigned => palette.status_info,
        EventType::WorkerCompleted => palette.status_success,
        EventType::SupervisorNotified => palette.status_warning,
        EventType::SupervisorInjected => palette.accent,
        // Worker activity events
        EventType::WorkerSubagentSpawned => palette.status_info,
        EventType::WorkerSubagentCompleted => palette.status_success,
        EventType::WorkerFileEdited => palette.text_primary,
        EventType::WorkerGitCommit => palette.accent,
        EventType::WorkerVerificationBlocked => palette.status_warning,
        EventType::WorkerPushBlocked => palette.status_error,
        EventType::EpicSubtasksComplete => palette.status_success,
        // Verification lifecycle events
        EventType::VerificationStarted => palette.status_info,
        EventType::VerificationAdded => palette.status_success,
        // Audit / integrity events
        EventType::AuditTrailGap => palette.status_error,
    }
}

#[cfg(test)]
mod tests {
    use super::{truncate, truncate_to_width};

    #[test]
    fn truncate_handles_unicode_boundary() {
        let value = format!("{}✅ trailing", "a".repeat(99));
        assert_eq!(truncate(&value, 103), format!("{}...", "a".repeat(99)));
    }

    #[test]
    fn truncate_to_width_handles_unicode_boundary() {
        let value = format!("{}✅ trailing", "a".repeat(99));
        // width=106, prefix_chars=0 => available=103 => cut point would be byte 100 without boundary check
        assert_eq!(
            truncate_to_width(&value, 106, 0),
            format!("{}...", "a".repeat(99))
        );
    }
}
