//! Core data types for CAS (Coding Agent System)
//!
//! This module contains the fundamental data structures used throughout CAS:
//!
//! - [`Entry`] - Memory entries that store learnings, preferences, and observations
//! - [`Rule`] - Codified patterns that sync to Claude Code rules
//! - [`Task`] - Work items with dependencies for project tracking
//! - [`Skill`] - Agent capabilities that sync to Claude Code skills
//! - [`Dependency`] - Relationships between tasks (blocking, parent-child, etc.)
//!
//! # Memory Tiers
//!
//! Entries are organized into memory tiers (inspired by MemGPT):
//! - **InContext** - Always injected into sessions (pinned)
//! - **Working** - Active, readily accessible memories
//! - **Cold** - Less frequently accessed, may be compressed
//! - **Archive** - Archived for reference, not actively used
//!
//! # Example
//!
//! ```rust,ignore
//! use cas::types::{Entry, EntryType, MemoryTier};
//!
//! let entry = Entry {
//!     id: "2024-01-15-001".to_string(),
//!     entry_type: EntryType::Learning,
//!     content: "Always use table-driven tests in Go".to_string(),
//!     memory_tier: MemoryTier::Working,
//!     ..Default::default()
//! };
//! ```

pub mod error;

mod agent;
mod code_review;
mod commit_link;
mod dependency;
mod entity;
mod entry;
mod event;
mod file_change;
mod lease;
mod loop_state;
mod prompt;
mod recording;
mod rule;
mod scope;
mod search_manifest;
mod session;
mod skill;
mod sort;
mod spec;
mod preview;
mod task;
mod verification;
mod worktree;

pub use agent::{
    Agent, AgentCapability, AgentRole, AgentStatus, AgentType, DEFAULT_HEARTBEAT_INTERVAL_SECS,
    DEFAULT_HEARTBEAT_TIMEOUT_SECS, DEFAULT_LEASE_DURATION_SECS, DEFAULT_MAX_CONCURRENT_TASKS,
};
pub use code_review::{
    AutofixClass, FINDING_OPTIONAL_FIELDS, FINDING_REQUIRED_FIELDS, Finding,
    FindingValidationError, MAX_TITLE_LEN, Owner, ReviewOutcome, ReviewOutcomeParseError,
    ReviewerOutput, Severity as FindingSeverity, parse_review_outcome, parse_reviewer_output,
    review_outcome_shape_hint,
};
pub use commit_link::CommitLink;
pub use dependency::{Dependency, DependencyType};
pub use entity::{Entity, EntityMention, EntityType, RelationType, Relationship};
pub use entry::{BeliefType, Entry, EntryType, MemoryTier, ObservationType};
pub use event::{Event, EventEntityType, EventType};
pub use file_change::{ChangeType, FileChange};
pub use lease::{ClaimResult, LeaseStatus, TaskLease, WorktreeClaimResult, WorktreeLease};
pub use loop_state::{Loop, LoopStatus};
pub use prompt::{AgentInfo, Message, MessageRole, Prompt};
pub use recording::{
    Recording, RecordingAgent, RecordingEvent, RecordingEventType, RecordingQuery,
};
pub use rule::{Rule, RuleCategory, RuleStatus};
pub use scope::{Scope, ScopeFilter, ShareScope};
pub use search_manifest::{
    SearchManifest, SearchManifestEntry, parse_search_manifest, search_manifest_shape_hint,
    zero_hit_entries,
};
pub use session::{Session, SessionOutcome};
pub use skill::{Skill, SkillHookConfig, SkillHookEntry, SkillHooks, SkillStatus, SkillType};
pub use sort::{
    EntrySortField, EntrySortOptions, SearchSortField, SearchSortOptions, SortOrder, TaskSortField,
    TaskSortOptions,
};
pub use spec::{Spec, SpecStatus, SpecType};
pub use task::{Priority, Task, TaskDeliverables, TaskDepth, TaskStatus, TaskType};
pub use verification::{
    IssueSeverity, Verification, VerificationIssue, VerificationProvenance, VerificationStatus,
    VerificationType, VerifierCapability,
};
pub use worktree::{GitContext, Worktree, WorktreeStatus};

// Re-export error types
pub use error::{Result, TypeError};
