use crate::hooks::handlers::*;

// =========================================================================
// compute_session_title tests (cas-ae09)
// =========================================================================

/// Worker with an active in-progress task emits sessionTitle with role marker,
/// task ID, and a truncated title preview.
#[test]
fn worker_with_active_task_emits_title() {
    let mut task = Task::new("cas-1234".into(), "Implement the thing".into());
    task.status = TaskStatus::InProgress;

    let title = compute_session_title("worker", &[task]);
    let title = title.expect("worker with task must emit Some title");

    assert!(
        title.contains("[worker]"),
        "title must contain role marker [worker]: {title}"
    );
    assert!(
        title.contains("cas-1234"),
        "title must contain task id: {title}"
    );
    assert!(
        title.contains("Implement the thing"),
        "title must contain task title preview: {title}"
    );
}

/// Worker with no active task emits "[worker] idle".
#[test]
fn worker_idle_emits_idle_title() {
    let title = compute_session_title("worker", &[]);
    let title = title.expect("worker idle must emit Some title");
    assert_eq!(title, "[worker] idle", "idle worker must emit exact idle label");
}

/// Supervisor with an in-progress epic task emits "[supervisor] <epic-id>".
#[test]
fn supervisor_with_epic_emits_epic_title() {
    let mut epic = Task::new("cas-epic1".into(), "My Epic".into());
    epic.task_type = TaskType::Epic;
    epic.status = TaskStatus::InProgress;

    let title = compute_session_title("supervisor", &[epic]);
    let title = title.expect("supervisor with epic must emit Some title");

    assert!(
        title.contains("[supervisor]"),
        "title must contain role marker [supervisor]: {title}"
    );
    assert!(
        title.contains("cas-epic1"),
        "title must contain epic id: {title}"
    );
}

/// Supervisor with no in-progress epic falls back to "[supervisor] factory".
#[test]
fn supervisor_no_epic_emits_factory_title() {
    let title = compute_session_title("supervisor", &[]);
    let title = title.expect("supervisor with no tasks must emit Some title");
    assert_eq!(
        title, "[supervisor] factory",
        "supervisor fallback must be '[supervisor] factory'"
    );
}

/// Supervisor with only non-epic in-progress tasks falls back to "factory".
#[test]
fn supervisor_non_epic_task_falls_back_to_factory() {
    let mut task = Task::new("cas-9999".into(), "some task".into());
    task.task_type = TaskType::Task;
    task.status = TaskStatus::InProgress;

    let title = compute_session_title("supervisor", &[task]);
    let title = title.expect("supervisor must always emit Some");
    assert!(
        title.ends_with("factory"),
        "non-epic in-progress task must fall back to 'factory': {title}"
    );
}

/// Non-factory sessions (no role / unknown role) emit None — sessionTitle absent.
#[test]
fn non_factory_role_emits_no_title() {
    assert!(
        compute_session_title("", &[]).is_none(),
        "empty role must emit None"
    );
    assert!(
        compute_session_title("unknown", &[]).is_none(),
        "unknown role must emit None"
    );
}

// =========================================================================
// sessionTitle JSON serialization tests (cas-ae09)
// =========================================================================

/// `with_session_title` on an existing SessionStart output adds
/// `"sessionTitle"` to the JSON wire shape.
#[test]
fn session_title_serializes_in_session_start() {
    let output =
        HookOutput::with_session_start_context("ctx".into()).with_session_title("[worker] cas-1234 · Build thing".into());
    let json = serde_json::to_string(&output).unwrap();
    assert!(
        json.contains("\"sessionTitle\""),
        "Expected sessionTitle key in: {json}"
    );
    assert!(
        json.contains("[worker] cas-1234"),
        "Expected sessionTitle value in: {json}"
    );
    // additionalContext must still be present
    assert!(
        json.contains("\"additionalContext\":\"ctx\""),
        "additionalContext must still be emitted: {json}"
    );
}

/// `with_session_title` on an empty HookOutput creates a minimal SessionStart.
#[test]
fn session_title_on_empty_output_creates_session_start() {
    let output = HookOutput::empty().with_session_title("[worker] idle".into());
    let json = serde_json::to_string(&output).unwrap();
    assert!(
        json.contains("\"sessionTitle\":\"[worker] idle\""),
        "Expected sessionTitle in: {json}"
    );
    assert!(
        json.contains("SessionStart"),
        "Expected SessionStart hookEventName: {json}"
    );
}

/// sessionTitle is absent when not set — skip_serializing_if = Option::is_none.
#[test]
fn session_title_absent_by_default() {
    let output = HookOutput::with_session_start_context("ctx".into());
    let json = serde_json::to_value(&output).unwrap();
    assert!(json["hookSpecificOutput"].get("sessionTitle").is_none());
}

/// Long task titles are truncated to ~40 chars in the sessionTitle.
#[test]
fn worker_title_truncates_long_task_title() {
    let long_title = "a".repeat(80);
    let mut task = Task::new("cas-x".into(), long_title.clone());
    task.status = TaskStatus::InProgress;

    let title = compute_session_title("worker", &[task]).unwrap();
    // The preview portion (after " · ") should be at most 43 chars (40 + "...")
    let preview_part = title
        .splitn(2, " · ")
        .nth(1)
        .unwrap_or("");
    assert!(
        preview_part.len() <= 43,
        "title preview must be truncated to ≤43 chars, got {}: {}",
        preview_part.len(),
        preview_part
    );
}
