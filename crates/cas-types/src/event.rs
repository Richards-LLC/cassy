//! Event type definitions for activity tracking
//!
//! Events record significant actions in CAS for the sidecar activity feed.
//! They provide a chronological log of agent activity, task changes, and memory storage.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::error::TypeError;

/// Type of event that occurred
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventType {
    /// Agent registered with CAS
    AgentRegistered,
    /// Agent heartbeat (throttled, not every heartbeat)
    AgentHeartbeat,
    /// Agent shut down or died
    AgentShutdown,
    /// New task created
    TaskCreated,
    /// Task started (status → InProgress)
    TaskStarted,
    /// Task completed (status → Closed)
    TaskCompleted,
    /// Task blocked (status → Blocked)
    TaskBlocked,
    /// Note added to a task
    TaskNoteAdded,
    /// Task deleted
    TaskDeleted,
    /// Memory/learning stored
    MemoryStored,
    /// Rule promoted to proven
    RulePromoted,
    /// Skill used/invoked
    SkillUsed,

    // Factory session events
    /// Factory session started
    FactoryStarted,
    /// Factory session stopped
    FactoryStopped,
    /// Worker agent died unexpectedly
    WorkerDied,
    /// Worker assigned a task by supervisor
    WorkerAssigned,
    /// Worker completed assigned task
    WorkerCompleted,
    /// Supervisor received batch of notifications
    SupervisorNotified,
    /// Supervisor injected prompt to worker
    SupervisorInjected,

    // Worker activity events (for supervisor visibility)
    /// Worker spawned a subagent (e.g., task-verifier)
    WorkerSubagentSpawned,
    /// Worker's subagent completed
    WorkerSubagentCompleted,
    /// Worker edited a file
    WorkerFileEdited,
    /// Worker made a git commit
    WorkerGitCommit,
    /// Worker blocked waiting for verification
    WorkerVerificationBlocked,
    /// Worker push was refused by the factory branch guard
    WorkerPushBlocked,

    /// All subtasks of an epic are closed — epic ready to close
    EpicSubtasksComplete,

    // Verification lifecycle events
    /// Verification started (task-verifier spawned for a task)
    VerificationStarted,
    /// Verification result recorded
    VerificationAdded,

    // Audit / integrity events
    /// Worker-owned verification Skipped row could not be written — audit trail gap
    AuditTrailGap,
}

impl fmt::Display for EventType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventType::AgentRegistered => write!(f, "agent_registered"),
            EventType::AgentHeartbeat => write!(f, "agent_heartbeat"),
            EventType::AgentShutdown => write!(f, "agent_shutdown"),
            EventType::TaskCreated => write!(f, "task_created"),
            EventType::TaskStarted => write!(f, "task_started"),
            EventType::TaskCompleted => write!(f, "task_completed"),
            EventType::TaskBlocked => write!(f, "task_blocked"),
            EventType::TaskNoteAdded => write!(f, "task_note_added"),
            EventType::TaskDeleted => write!(f, "task_deleted"),
            EventType::MemoryStored => write!(f, "memory_stored"),
            EventType::RulePromoted => write!(f, "rule_promoted"),
            EventType::SkillUsed => write!(f, "skill_used"),
            EventType::FactoryStarted => write!(f, "factory_started"),
            EventType::FactoryStopped => write!(f, "factory_stopped"),
            EventType::WorkerDied => write!(f, "worker_died"),
            EventType::WorkerAssigned => write!(f, "worker_assigned"),
            EventType::WorkerCompleted => write!(f, "worker_completed"),
            EventType::SupervisorNotified => write!(f, "supervisor_notified"),
            EventType::SupervisorInjected => write!(f, "supervisor_injected"),
            EventType::WorkerSubagentSpawned => write!(f, "worker_subagent_spawned"),
            EventType::WorkerSubagentCompleted => write!(f, "worker_subagent_completed"),
            EventType::WorkerFileEdited => write!(f, "worker_file_edited"),
            EventType::WorkerGitCommit => write!(f, "worker_git_commit"),
            EventType::WorkerVerificationBlocked => write!(f, "worker_verification_blocked"),
            EventType::WorkerPushBlocked => write!(f, "worker_push_blocked"),
            EventType::EpicSubtasksComplete => write!(f, "epic_subtasks_complete"),
            EventType::VerificationStarted => write!(f, "verification_started"),
            EventType::VerificationAdded => write!(f, "verification_added"),
            EventType::AuditTrailGap => write!(f, "audit_trail_gap"),
        }
    }
}

impl FromStr for EventType {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "agent_registered" => Ok(EventType::AgentRegistered),
            "agent_heartbeat" => Ok(EventType::AgentHeartbeat),
            "agent_shutdown" => Ok(EventType::AgentShutdown),
            "task_created" => Ok(EventType::TaskCreated),
            "task_started" => Ok(EventType::TaskStarted),
            "task_completed" => Ok(EventType::TaskCompleted),
            "task_blocked" => Ok(EventType::TaskBlocked),
            "task_note_added" => Ok(EventType::TaskNoteAdded),
            "task_deleted" => Ok(EventType::TaskDeleted),
            "memory_stored" => Ok(EventType::MemoryStored),
            "rule_promoted" => Ok(EventType::RulePromoted),
            "skill_used" => Ok(EventType::SkillUsed),
            "factory_started" => Ok(EventType::FactoryStarted),
            "factory_stopped" => Ok(EventType::FactoryStopped),
            "worker_died" => Ok(EventType::WorkerDied),
            "worker_assigned" => Ok(EventType::WorkerAssigned),
            "worker_completed" => Ok(EventType::WorkerCompleted),
            "supervisor_notified" => Ok(EventType::SupervisorNotified),
            "supervisor_injected" => Ok(EventType::SupervisorInjected),
            "worker_subagent_spawned" => Ok(EventType::WorkerSubagentSpawned),
            "worker_subagent_completed" => Ok(EventType::WorkerSubagentCompleted),
            "worker_file_edited" => Ok(EventType::WorkerFileEdited),
            "worker_git_commit" => Ok(EventType::WorkerGitCommit),
            "worker_verification_blocked" => Ok(EventType::WorkerVerificationBlocked),
            "worker_push_blocked" => Ok(EventType::WorkerPushBlocked),
            "epic_subtasks_complete" => Ok(EventType::EpicSubtasksComplete),
            "verification_started" => Ok(EventType::VerificationStarted),
            "verification_added" => Ok(EventType::VerificationAdded),
            "audit_trail_gap" => Ok(EventType::AuditTrailGap),
            _ => Err(TypeError::Parse(format!("invalid event type: {s}"))),
        }
    }
}

/// Type of entity the event relates to
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EventEntityType {
    Agent,
    Task,
    Entry,
    Rule,
    Skill,
    Session,
    Verification,
}

impl fmt::Display for EventEntityType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            EventEntityType::Agent => write!(f, "agent"),
            EventEntityType::Task => write!(f, "task"),
            EventEntityType::Entry => write!(f, "entry"),
            EventEntityType::Rule => write!(f, "rule"),
            EventEntityType::Skill => write!(f, "skill"),
            EventEntityType::Session => write!(f, "session"),
            EventEntityType::Verification => write!(f, "verification"),
        }
    }
}

impl FromStr for EventEntityType {
    type Err = TypeError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "agent" => Ok(EventEntityType::Agent),
            "task" => Ok(EventEntityType::Task),
            "entry" => Ok(EventEntityType::Entry),
            "rule" => Ok(EventEntityType::Rule),
            "skill" => Ok(EventEntityType::Skill),
            "session" => Ok(EventEntityType::Session),
            "verification" => Ok(EventEntityType::Verification),
            _ => Err(TypeError::Parse(format!("invalid entity type: {s}"))),
        }
    }
}

/// An event recording a significant action in CAS
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Event {
    /// Auto-incrementing event ID
    pub id: i64,
    /// Type of event
    pub event_type: EventType,
    /// Type of entity this event relates to
    pub entity_type: EventEntityType,
    /// ID of the affected entity
    pub entity_id: String,
    /// Human-readable summary of the event
    pub summary: String,
    /// Optional JSON metadata with additional context
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
    /// When the event occurred
    pub created_at: DateTime<Utc>,
    /// Optional session ID of the agent that caused this event
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl Event {
    /// Create a new event (ID will be assigned by the database)
    pub fn new(
        event_type: EventType,
        entity_type: EventEntityType,
        entity_id: impl Into<String>,
        summary: impl Into<String>,
    ) -> Self {
        Self {
            id: 0, // Will be set by database
            event_type,
            entity_type,
            entity_id: entity_id.into(),
            summary: summary.into(),
            metadata: None,
            created_at: Utc::now(),
            session_id: None,
        }
    }

    /// Add metadata to the event
    pub fn with_metadata(mut self, metadata: serde_json::Value) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Add session ID to the event
    pub fn with_session(mut self, session_id: impl Into<String>) -> Self {
        self.session_id = Some(session_id.into());
        self
    }

    /// Get the icon for this event type (for UI display)
    pub fn icon(&self) -> &'static str {
        match self.event_type {
            EventType::AgentRegistered => "●",            // Circle filled
            EventType::AgentHeartbeat => "◐",             // Circle half
            EventType::AgentShutdown => "○",              // Circle empty
            EventType::TaskCreated => "○",                // Open task
            EventType::TaskStarted => "◔",                // Spinner static
            EventType::TaskCompleted => "✓",              // Check
            EventType::TaskBlocked => "⛔",               // Blocked
            EventType::TaskNoteAdded => "•",              // Bullet
            EventType::TaskDeleted => "✗",                // X mark
            EventType::MemoryStored => "•",               // Bullet
            EventType::RulePromoted => "★",               // Star
            EventType::SkillUsed => "✨",                 // Sparkles
            EventType::FactoryStarted => "▶",             // Play
            EventType::FactoryStopped => "■",             // Stop
            EventType::WorkerDied => "✗",                 // Cross
            EventType::WorkerAssigned => "→",             // Arrow
            EventType::WorkerCompleted => "✓",            // Check
            EventType::SupervisorNotified => "↓",         // Down arrow (received)
            EventType::SupervisorInjected => "↑",         // Up arrow (sent)
            EventType::WorkerSubagentSpawned => "⚡",     // Lightning (spawning)
            EventType::WorkerSubagentCompleted => "✓",    // Check (completed)
            EventType::WorkerFileEdited => "✎",           // Pencil (edited)
            EventType::WorkerGitCommit => "⬆",            // Up arrow (commit)
            EventType::WorkerVerificationBlocked => "🔒", // Lock (blocked)
            EventType::WorkerPushBlocked => "🚫",         // No entry (push blocked)
            EventType::EpicSubtasksComplete => "🎉",       // Party (all subtasks done)
            EventType::VerificationStarted => "🔍",       // Magnifying glass (verifying)
            EventType::VerificationAdded => "📋",         // Clipboard (result recorded)
            EventType::AuditTrailGap => "⚠",             // Warning (audit gap)
        }
    }
}

impl Default for Event {
    fn default() -> Self {
        Self {
            id: 0,
            event_type: EventType::MemoryStored,
            entity_type: EventEntityType::Entry,
            entity_id: String::new(),
            summary: String::new(),
            metadata: None,
            created_at: Utc::now(),
            session_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::event::*;

    #[test]
    fn test_event_type_display() {
        assert_eq!(EventType::AgentRegistered.to_string(), "agent_registered");
        assert_eq!(EventType::TaskStarted.to_string(), "task_started");
    }

    #[test]
    fn test_event_type_from_str() {
        assert_eq!(
            EventType::from_str("agent_registered").unwrap(),
            EventType::AgentRegistered
        );
        assert_eq!(
            EventType::from_str("TASK_STARTED").unwrap(),
            EventType::TaskStarted
        );
    }

    /// cas-eeab: verify AuditTrailGap Display/FromStr round-trip and icon are
    /// consistent so a typo in either direction is caught at test time rather than
    /// silently misfiled in the daemon's event store.
    #[test]
    fn test_audit_trail_gap_round_trips() {
        let s = EventType::AuditTrailGap.to_string();
        assert_eq!(s, "audit_trail_gap");
        assert_eq!(
            s.parse::<EventType>().unwrap(),
            EventType::AuditTrailGap
        );
        // Icon must be non-empty (used by TUI icon column); icon() is on Event, not EventType
        let event = Event::new(
            EventType::AuditTrailGap,
            EventEntityType::Task,
            "cas-test",
            "test gap",
        );
        assert!(!event.icon().is_empty());
    }

    #[test]
    fn test_worker_push_blocked_round_trips() {
        let s = EventType::WorkerPushBlocked.to_string();
        assert_eq!(s, "worker_push_blocked");
        assert_eq!(s.parse::<EventType>().unwrap(), EventType::WorkerPushBlocked);
        assert_eq!(
            Event::new(
                EventType::WorkerPushBlocked,
                EventEntityType::Agent,
                "worker",
                "push blocked",
            )
            .icon(),
            "🚫"
        );
    }

    #[test]
    fn test_event_new() {
        let event = Event::new(
            EventType::TaskStarted,
            EventEntityType::Task,
            "cas-abc1",
            "Task started: Fix the bug",
        );
        assert_eq!(event.event_type, EventType::TaskStarted);
        assert_eq!(event.entity_id, "cas-abc1");
        assert_eq!(event.icon(), "◔");
    }

    #[test]
    fn test_event_with_metadata() {
        let event = Event::new(
            EventType::MemoryStored,
            EventEntityType::Entry,
            "2025-01-20-001",
            "Stored learning about Rust",
        )
        .with_metadata(serde_json::json!({"tags": ["rust", "learning"]}))
        .with_session("session-123");

        assert!(event.metadata.is_some());
        assert_eq!(event.session_id, Some("session-123".to_string()));
    }

    #[test]
    fn test_session_entity_type() {
        assert_eq!(
            EventEntityType::from_str("session").unwrap(),
            EventEntityType::Session
        );
        assert_eq!(EventEntityType::Session.to_string(), "session");
    }
}
