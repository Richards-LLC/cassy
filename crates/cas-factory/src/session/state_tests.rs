use std::path::PathBuf;

use crate::session::state::*;

#[test]
fn test_session_id_format() {
    let id = generate_session_id();
    assert!(id.starts_with("fs-"));
    assert!(id.len() > 10);
}

// === SessionType tests ===

#[test]
fn test_session_type_display() {
    assert_eq!(SessionType::Factory.to_string(), "factory");
    assert_eq!(SessionType::Managed.to_string(), "managed");
    assert_eq!(SessionType::Recording.to_string(), "recording");
}

#[test]
fn test_session_type_serde() {
    let factory = SessionType::Factory;
    let json = serde_json::to_string(&factory).unwrap();
    assert_eq!(json, "\"factory\"");

    let parsed: SessionType = serde_json::from_str("\"managed\"").unwrap();
    assert_eq!(parsed, SessionType::Managed);
}

// === AgentState tests ===

#[test]
fn test_agent_state_display() {
    assert_eq!(AgentState::Active.to_string(), "active");
    assert_eq!(AgentState::Idle.to_string(), "idle");
    assert_eq!(AgentState::Blocked.to_string(), "blocked");
    assert_eq!(AgentState::Exited.to_string(), "exited");
}

#[test]
fn test_agent_state_serde() {
    let active = AgentState::Active;
    let json = serde_json::to_string(&active).unwrap();
    assert_eq!(json, "\"active\"");

    let parsed: AgentState = serde_json::from_str("\"blocked\"").unwrap();
    assert_eq!(parsed, AgentState::Blocked);
}

// === SessionSummary tests ===

#[test]
fn test_session_summary_new() {
    let summary = SessionSummary::new(
        "test-123".to_string(),
        SessionType::Factory,
        PathBuf::from("/project"),
    );

    assert_eq!(summary.id, "test-123");
    assert_eq!(summary.session_type, SessionType::Factory);
    assert_eq!(summary.state, SessionState::Active);
    assert_eq!(summary.worker_count, 0);
    assert!(summary.supervisor_name.is_none());
    assert_eq!(summary.project_dir, PathBuf::from("/project"));
    assert!(!summary.recording_enabled);
    assert!(summary.epic_id.is_none());
    assert_eq!(summary.total_output_bytes, 0);
    assert!(summary.agent_states.is_empty());
}

#[test]
fn test_session_summary_factory() {
    let summary = SessionSummary::factory(
        "factory-1".to_string(),
        PathBuf::from("/project"),
        Some("supervisor-1".to_string()),
    );

    assert!(summary.is_factory());
    assert!(!summary.is_managed());
    assert!(!summary.is_recording());
    assert_eq!(summary.supervisor_name, Some("supervisor-1".to_string()));
}

#[test]
fn test_session_summary_managed() {
    let summary = SessionSummary::managed("managed-1".to_string(), PathBuf::from("/project"));

    assert!(!summary.is_factory());
    assert!(summary.is_managed());
    assert!(!summary.is_recording());
}

#[test]
fn test_session_summary_recording() {
    let summary = SessionSummary::recording("recording-1".to_string(), PathBuf::from("/project"));

    assert!(!summary.is_factory());
    assert!(!summary.is_managed());
    assert!(summary.is_recording());
    assert!(summary.recording_enabled);
}

#[test]
fn test_session_summary_agent_states() {
    let mut summary = SessionSummary::factory(
        "test".to_string(),
        PathBuf::from("/project"),
        Some("supervisor".to_string()),
    );

    // Add agents
    summary.set_agent_state("worker-1", AgentState::Active);
    summary.set_agent_state("worker-2", AgentState::Idle);
    summary.set_agent_state("worker-3", AgentState::Blocked);

    assert_eq!(summary.worker_count, 3);
    assert_eq!(summary.active_agent_count(), 3);
    assert_eq!(
        summary.agent_states.get("worker-1"),
        Some(&AgentState::Active)
    );
    assert_eq!(
        summary.agent_states.get("worker-2"),
        Some(&AgentState::Idle)
    );

    // Exit an agent - should remove from map and update count
    summary.set_agent_state("worker-1", AgentState::Exited);
    assert_eq!(summary.worker_count, 2);
    assert_eq!(summary.active_agent_count(), 2);
    assert!(!summary.agent_states.contains_key("worker-1"));
}

#[test]
fn test_session_summary_output_bytes() {
    let mut summary = SessionSummary::default();

    summary.add_output_bytes(100);
    assert_eq!(summary.total_output_bytes, 100);

    summary.add_output_bytes(250);
    assert_eq!(summary.total_output_bytes, 350);

    // Test saturation (shouldn't overflow)
    summary.total_output_bytes = u64::MAX - 10;
    summary.add_output_bytes(100);
    assert_eq!(summary.total_output_bytes, u64::MAX);
}

#[test]
fn test_session_summary_set_epic() {
    let mut summary = SessionSummary::default();
    assert!(summary.epic_id.is_none());

    summary.set_epic("epic-123");
    assert_eq!(summary.epic_id, Some("epic-123".to_string()));
}

#[test]
fn test_session_summary_touch() {
    let mut summary = SessionSummary::default();
    let initial = summary.last_activity;

    std::thread::sleep(std::time::Duration::from_millis(10));
    summary.touch();

    assert!(summary.last_activity > initial);
}

#[test]
fn test_session_summary_serde() {
    let mut summary = SessionSummary::factory(
        "test-serde".to_string(),
        PathBuf::from("/project"),
        Some("supervisor".to_string()),
    );
    summary.set_epic("epic-1");
    summary.set_agent_state("worker-1", AgentState::Active);
    summary.add_output_bytes(1024);

    let json = serde_json::to_string(&summary).unwrap();
    let parsed: SessionSummary = serde_json::from_str(&json).unwrap();

    assert_eq!(parsed.id, "test-serde");
    assert_eq!(parsed.session_type, SessionType::Factory);
    assert_eq!(parsed.supervisor_name, Some("supervisor".to_string()));
    assert_eq!(parsed.epic_id, Some("epic-1".to_string()));
    assert_eq!(parsed.worker_count, 1);
    assert_eq!(parsed.total_output_bytes, 1024);
    assert_eq!(
        parsed.agent_states.get("worker-1"),
        Some(&AgentState::Active)
    );
}
