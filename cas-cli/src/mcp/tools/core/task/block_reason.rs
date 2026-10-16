//! Durable task-note signals that explain why a task remains blocked.
//!
//! Dependency edges explain dependency blocks. Operator holds live in the
//! task's existing append-only notes so automatic dependency transitions can
//! distinguish them without adding another persisted task field or migration.

use cas_types::Task;

const EXPLICIT_STATUS_HOLD: &str = "🚫 BLOCKED BY STATUS:";
const EXPLICIT_HOLD_CLEAR: &str = "🔓 BLOCK HOLD CLEARED:";

/// Return the active non-dependency hold recorded in `notes`, if any.
///
/// Typed blocker notes always hold a task. A decision note holds it only when
/// its text explicitly says to hold, wait, pause, defer, or keep the task
/// blocked. Explicit `status=blocked` updates append a dedicated hold record.
/// An explicit unblock or a prior auto-unblock clears older hold records.
pub(crate) fn non_dependency_hold_reason(notes: &str) -> Option<String> {
    let mut holds = Vec::new();

    for line in notes.lines() {
        let trimmed = line.trim();
        if is_hold_clear(trimmed) {
            holds.clear();
            continue;
        }

        if let Some(reason) = after_marker(trimmed, EXPLICIT_STATUS_HOLD) {
            push_unique(&mut holds, reason);
            continue;
        }
        if let Some(reason) = after_marker(trimmed, "🚫 BLOCKER")
            .or_else(|| after_marker(trimmed, "BLOCKER:"))
            .or_else(|| after_marker(trimmed, "blocker:"))
        {
            push_unique(&mut holds, reason);
            continue;
        }

        let decision = after_marker(trimmed, "✅ DECISION")
            .or_else(|| after_marker(trimmed, "DECISION:"))
            .or_else(|| after_marker(trimmed, "decision:"));
        if let Some(reason) = decision.filter(|reason| explicitly_holds(reason)) {
            push_unique(&mut holds, reason);
            continue;
        }

        if explicitly_holds(trimmed) {
            push_unique(&mut holds, without_timestamp_prefix(trimmed).to_string());
        }
    }

    (!holds.is_empty()).then(|| holds.join("; "))
}

/// Record a caller's explicit `status=blocked` intent, even when dependencies
/// had already projected the task to Blocked.
pub(crate) fn record_explicit_status_hold(task: &mut Task, reason: Option<&str>) {
    let reason = reason
        .map(str::trim)
        .filter(|reason| !reason.is_empty())
        .unwrap_or("Task explicitly set to status=blocked.");
    append_note(
        task,
        &format!("{EXPLICIT_STATUS_HOLD} {}", one_line(reason)),
    );
}

/// Record that a supervisor explicitly released the task's existing holds.
pub(crate) fn record_hold_clear(task: &mut Task, reason: &str) {
    append_note(task, &format!("{EXPLICIT_HOLD_CLEAR} {}", one_line(reason)));
}

/// Append an explanation when dependency edges clear but an operator hold is
/// still active. The prose is deliberately not itself a hold marker.
pub(crate) fn record_auto_unblock_skipped(task: &mut Task, reason: &str) {
    append_note(
        task,
        &format!(
            "⏸ Auto-unblock skipped: dependency blockers cleared; task remains blocked because {reason}."
        ),
    );
}

fn append_note(task: &mut Task, body: &str) {
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
    let note = format!("[{timestamp}] {body}");
    if task.notes.is_empty() {
        task.notes = note;
    } else {
        task.notes.push_str("\n\n");
        task.notes.push_str(&note);
    }
    task.updated_at = chrono::Utc::now();
}

fn after_marker(line: &str, marker: &str) -> Option<String> {
    let index = line.find(marker)?;
    let reason = line[index + marker.len()..]
        .trim()
        .trim_start_matches(':')
        .trim();
    (!reason.is_empty()).then(|| one_line(reason))
}

fn is_hold_clear(line: &str) -> bool {
    line.contains(EXPLICIT_HOLD_CLEAR)
        || without_timestamp_prefix(line).starts_with("Auto-unblocked:")
}

fn explicitly_holds(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    [
        "operator hold",
        "operator held",
        "hold until",
        "held until",
        "hold for",
        "held for",
        "on hold",
        "keep blocked",
        "leave blocked",
        "remain blocked",
        "stay blocked",
        "wait for",
        "waiting for",
        "pause until",
        "paused until",
        "defer until",
        "deferred until",
    ]
    .iter()
    .any(|phrase| text.contains(phrase))
}

fn push_unique(holds: &mut Vec<String>, reason: String) {
    if !holds
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(&reason))
    {
        holds.push(reason);
    }
}

fn without_timestamp_prefix(line: &str) -> &str {
    line.strip_prefix('[')
        .and_then(|line| line.split_once("] "))
        .map_or(line, |(_, body)| body)
}

fn one_line(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\n' | '\r' => ' ',
            other => other,
        })
        .take(300)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::non_dependency_hold_reason;

    #[test]
    fn decision_and_explicit_status_holds_survive_until_unblocked() {
        let notes = "[2026-10-16 10:00] ✅ DECISION Hold until Slack ownership is decided.\n\
                     [2026-10-16 10:01] 🚫 BLOCKED BY STATUS: operator explicitly paused this task.\n\
                     [2026-10-16 10:02] 🔓 BLOCK HOLD CLEARED: operator resumed work.";
        assert_eq!(non_dependency_hold_reason(notes), None);
    }

    #[test]
    fn latest_hold_reason_is_surfaced_and_dependency_notes_do_not_hold() {
        let notes = "Dependency changed: blocker added.\n\
                     [2026-10-16 10:00] ✅ DECISION Hold until Slack ownership is decided.";
        assert_eq!(
            non_dependency_hold_reason(notes).as_deref(),
            Some("Hold until Slack ownership is decided.")
        );
    }

    #[test]
    fn blocker_and_explicit_status_notes_are_holds() {
        let blocker = "[2026-10-16 10:00] 🚫 BLOCKER Operator hold: wait for the owner.";
        assert_eq!(
            non_dependency_hold_reason(blocker).as_deref(),
            Some("Operator hold: wait for the owner.")
        );

        let status = "[2026-10-16 10:00] 🚫 BLOCKED BY STATUS: supervisor paused work.";
        assert_eq!(
            non_dependency_hold_reason(status).as_deref(),
            Some("supervisor paused work.")
        );
    }
}
