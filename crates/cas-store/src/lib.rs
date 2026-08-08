//! Storage abstraction for CAS
//!
//! This crate provides a unified storage interface for all CAS data types:
//!
//! - [`Store`] - Entry (memory) storage operations
//! - [`RuleStore`] - Rule storage operations
//! - [`TaskStore`] - Task and dependency storage operations
//! - [`SkillStore`] - Skill storage operations
//! - [`CodeStore`] - Code file and symbol storage operations
//!
//! # Implementations
//!
//! - **SQLite** (`SqliteStore`, `SqliteRuleStore`, etc.) - Primary backend,
//!   uses WAL mode for good read concurrency
//! - **Markdown** (`MarkdownStore`, `MarkdownRuleStore`) - Legacy file-based
//!   storage with YAML frontmatter
//!
//! # Concurrency
//!
//! All SQLite stores share a process-level connection pool (`shared_db`) to
//! minimize connection count. A 5-second busy timeout plus application-level
//! retry with exponential backoff handles concurrent access from multiple
//! agents in factory mode.

use chrono::{DateTime, Utc};
use std::time::Duration;

/// Busy timeout for SQLite connections to handle concurrent access.
/// When multiple agents try to write simultaneously, SQLite will wait
/// up to this duration before returning SQLITE_BUSY.
///
/// Reduced from 30s to 5s — application-level retry with exponential
/// backoff (in `shared_db::with_write_retry`) handles longer contention.
pub const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);

pub mod shared_db;

mod agent_store;
mod code_store;
mod commit_link_store;
mod delivery_store;
mod entity_store;
pub mod error;
mod event_store;
mod file_change_store;
mod fts_query;
mod history_store;
mod knowledge_store;
pub mod layered;
mod loop_store;
pub mod markdown;
mod prompt_queue_store;
mod prompt_store;
mod recording_store;
mod recording_text_store;
mod reminder_store;
mod retrieval_store;
mod skill_store;
mod spawn_queue_store;
mod spec_store;
mod sqlite;
mod sqlite_code_store;
mod supervisor_queue_store;
mod task_store;
pub mod tracing;
mod verification_store;
mod worktree_store;

pub mod code_review;

// Mock stores for testing
#[cfg(test)]
pub mod mock;

// Worktree lease tests (TDD)
#[cfg(test)]
mod worktree_lease_test;

// Re-export error types
pub use error::{Result, StoreError};

// Agent store for multi-agent coordination
pub use agent_store::{AGENT_SCHEMA, AgentStore, LeaseHistoryEntry, SqliteAgentStore};

// Event store for activity tracking (sidecar)
pub use event_store::{EVENT_SCHEMA, EventStore, SqliteEventStore, record_event_with_conn};

// Code store for indexed source code
pub use code_store::CodeStore;
pub use delivery_store::{
    DELIVERY_SCHEMA, build_worker_completion_receipt, create_worker_delivery,
    create_worker_delivery_with_dispatch, create_worker_delivery_with_dispatch_for_lease,
    get_latest_worker_delivery,
    get_worker_delivery_by_receipt, list_worker_delivery_events, transition_worker_delivery,
    transition_worker_delivery_verification_with_conn, worker_delivery_transaction_id,
};
pub use sqlite_code_store::{CODE_SCHEMA, SqliteCodeStore};

// Entity store for knowledge graph feature
pub use entity_store::{ENTITY_SCHEMA, SqliteEntityStore};

// Structural git-history index (EPIC cas-6212 / cas-7a21): commits, their
// touched files, and the walker watermark.
pub use history_store::{
    CoChangedFile, DOC_KIND_CHANGELOG, DOC_KIND_COMMENT, DOC_KIND_ISSUE, DOC_KIND_PR,
    HISTORY_DOCS_SCHEMA, HISTORY_DOCS_SCHEMA_STATEMENTS, HISTORY_FTS_STATEMENTS, HISTORY_SCHEMA,
    HISTORY_SCHEMA_STATEMENTS, HistoryCommit, HistoryCommitFile, HistoryCommitHit, HistoryDoc,
    HistoryIndexState, HistoryQuery, HistoryStore, ProvenanceCoverage, SOURCE_CHANGELOG,
    SOURCE_GIT, SOURCE_GITHUB, SqliteHistoryStore,
};

// Knowledge store for LLM-distilled repo prose (EPIC cas-7d31 / cas-cbf1):
// markdown bodies on disk, index + content-hash source ledger in cas.db.
pub use knowledge_store::{
    DiskSource, IngestBatch, IngestReason, IngestReport, KNOWLEDGE_DIR_NAME, KNOWLEDGE_SCHEMA,
    KNOWLEDGE_SCHEMA_STATEMENTS, KnowledgeHit, KnowledgePage, KnowledgeSource, KnowledgeStore,
    PageWrite, PendingSource, SourceClassification, SourceOutcome, SourceStatus,
    SqliteKnowledgeStore, blake3_hex, canonical_rel_path, classify_sources, hash_source_file,
    slugify,
};

// Loop store for iteration loops
pub use loop_store::{LOOP_SCHEMA, LoopStore, SqliteLoopStore};

// Verification store for task quality gates
pub use verification_store::{
    IssuedVerifierCapability, SqliteVerificationStore, VERIFICATION_SCHEMA, VerificationStore,
    add_system_verification, add_verification_with_conn, bind_server_verifier_handoff,
    bind_server_verifier_handoff_and_register_child, bind_verifier_capability,
    cancel_unbound_server_verifier_handoff, claim_verification_dispatch,
    claim_verification_dispatch_bound, consume_server_verifier_handoff_with_conn,
    consume_verifier_capability_with_conn, create_verification_dispatch,
    create_verification_dispatch_bound, create_verification_dispatch_bound_with_conn,
    get_latest_verification_dispatch, get_latest_verification_dispatch_with_conn,
    get_verification_dispatch, get_verification_dispatch_with_conn, get_verification_for_dispatch,
    inspect_bound_server_verifier_handoff, inspect_verifier_capability,
    invalidate_verification_dispatch_and_reopen_task_exact,
    invalidate_verification_dispatch_for_new_cycle, issue_server_verifier_handoff,
    issue_server_verifier_handoff_with_secret, issue_verifier_capability,
    resolve_verification_dispatch_with_conn, save_verification_issues_with_conn,
    reopen_closed_task_atomic, request_changes_for_parked_delivery,
    timeout_verification_dispatch, update_system_verification,
    ParentDependencyUpdate, RequestChangesBoundary, RequestChangesOutcome,
    TaskReopenLifecycleOutbox,
};

// Worktree store for git worktree tracking
pub use worktree_store::{SqliteWorktreeStore, WORKTREE_SCHEMA, WorktreeStore};

mod known_repo_store;
pub use known_repo_store::{KnownRepo, KnownRepoBinding, KnownRepoStore, SqliteKnownRepoStore};

// Recording store for terminal recording metadata
pub use recording_store::{
    RecordingStore, SqliteRecordingStore, capture_agent_event, capture_memory_event,
    capture_message_event, capture_recording_event, capture_task_event,
    get_active_recording_with_conn, get_any_active_recording_with_conn,
    record_agent_event_with_conn, record_memory_event_with_conn, record_message_event_with_conn,
    record_recording_event_with_conn, record_task_event_with_conn,
};

// Supervisor queue store for factory sessions
pub use supervisor_queue_store::{
    NotificationPriority, NotifyIdempotentResult, SqliteSupervisorQueueStore,
    SupervisorNotification, SupervisorQueueStore,
};

// Prompt queue store for supervisor → worker communication
// (includes enqueue outcomes for message dedup and cas-ecff lifecycle outbox)
pub use prompt_queue_store::{
    ConfirmationSource, DeliveryStage, EnqueueIdempotentResult, EnqueueOutcome,
    MessageDeliveryReport, MessageStatus,
    ObservationStatus, PROMPT_QUEUE_STALE_TTL_SECS, PROMPT_RETRY_MAX_AGE_SECS, PendingReason,
    PromptQueueStore, PromptRetryDisposition, QueuedPrompt, RetriedPrompt,
    SqlitePromptQueueStore, SurfacingSource, UndeliveredLifecycleRelay, WakeAttempt,
    reply_confirms_delivered_message,
};

// Reminder store for supervisor "Remind Me" feature
pub use reminder_store::{
    Reminder, ReminderExpiryOutcome, ReminderStatus, ReminderStore, ReminderTriggerType,
    SqliteReminderStore, expire_stale_bounded, format_reminder_delivery,
    parse_reminder_delivery_id,
};

// Retrieval provenance and explicit outcome feedback
pub use retrieval_store::{
    DEFAULT_RETRIEVAL_POLICY, RETRIEVAL_SCHEMA, RETRIEVAL_SCHEMA_STATEMENTS, RetrievalAggregate,
    RetrievalHitIdentity, RetrievalOutcome, RetrievalOutcomeEvent, RetrievalQuery, RetrievalStore,
    SqliteRetrievalStore,
};

// Spawn queue store for worker lifecycle commands
pub use spawn_queue_store::{
    SPAWN_QUEUE_SCHEMA, SpawnAction, SpawnLifecycle, SpawnLifecycleState, SpawnQueueStore,
    SpawnRequest, SqliteSpawnQueueStore,
};

// Prompt store for tracking user prompts (code attribution)
pub use prompt_store::{
    PROMPT_SCHEMA, PromptStore, SqlitePromptStore, add_prompt_with_conn,
    get_current_prompt_for_session,
};

// File change store for tracking AI-generated code changes (code attribution)
pub use file_change_store::{
    FILE_CHANGE_SCHEMA, FileChangeStore, SqliteFileChangeStore, add_file_change_with_conn,
};

// Commit link store for associating git commits with AI sessions (code attribution)
pub use commit_link_store::{
    COMMIT_LINK_SCHEMA, CommitLinkStore, SqliteCommitLinkStore, add_commit_link_with_conn,
};

// Recording text store for full-text search in factory recordings
pub use recording_text_store::{
    RecordingSearchResult, RecordingTextEntry, RecordingTextStore, SqliteRecordingTextStore,
    format_timestamp,
};

// Core stores
pub use layered::{LayeredEntryStore, LayeredRuleStore, LayeredSkillStore};
pub use markdown::{MarkdownRuleStore, MarkdownStore};
pub use skill_store::{SKILL_SCHEMA, SqliteSkillStore};
pub use spec_store::{SpecStore, SqliteSpecStore};
pub use sqlite::{ENTRIES_RULES_SCHEMA, SqliteRuleStore, SqliteStore};
pub use task_store::{SqliteTaskStore, TASK_SCHEMA, clear_pending_verification_with_conn};

use cas_types::{
    Dependency, DependencyType, Entity, EntityMention, EntityType, Entry, RelationType,
    Relationship, Rule, Scope, Skill, SkillStatus, Task, TaskStatus, TaskType,
};

/// Trait for entry storage operations
pub trait Store: Send + Sync {
    /// Initialize the store (create tables, etc.)
    fn init(&self) -> Result<()>;

    /// Generate a new unique entry ID
    fn generate_id(&self) -> Result<String>;

    /// Add a new entry
    fn add(&self, entry: &Entry) -> Result<()>;

    /// Get an entry by ID
    fn get(&self, id: &str) -> Result<Entry>;

    /// Get an archived entry by ID
    fn get_archived(&self, id: &str) -> Result<Entry>;

    /// Update an existing entry
    fn update(&self, entry: &Entry) -> Result<()>;

    /// Delete an entry
    fn delete(&self, id: &str) -> Result<()>;

    /// List all active (non-archived) entries
    fn list(&self) -> Result<Vec<Entry>>;

    /// List active entries matching a scope and tag.
    ///
    /// Default falls back to list() + filter for non-SQLite stores; SQLite
    /// overrides with a query-layer filter for SessionStart hot paths.
    fn list_by_scope_and_tag(&self, scope: Scope, tag: &str) -> Result<Vec<Entry>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|e| e.scope == scope)
            .filter(|e| e.tags.iter().any(|t| t.trim().eq_ignore_ascii_case(tag)))
            .collect())
    }

    /// List entries eligible for memory decay (not InContext or Archive tier).
    /// Default falls back to list() + filter; SQLite overrides with a filtered query.
    fn list_decayable(&self) -> Result<Vec<Entry>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|e| {
                e.memory_tier != cas_types::MemoryTier::InContext
                    && e.memory_tier != cas_types::MemoryTier::Archive
            })
            .collect())
    }

    /// List entries eligible for auto-pruning (low stability, not archived).
    /// Default falls back to list() + filter; SQLite overrides with a filtered query.
    fn list_prunable(&self, stability_threshold: f32) -> Result<Vec<Entry>> {
        Ok(self
            .list()?
            .into_iter()
            .filter(|e| e.stability < stability_threshold)
            .collect())
    }

    /// Get the N most recent entries
    fn recent(&self, n: usize) -> Result<Vec<Entry>>;

    /// Timestamp used to rank an entry returned by [`Store::recent`].
    ///
    /// Stores that do not track updates fall back to the creation time.
    fn recent_timestamp(&self, entry: &Entry) -> Result<DateTime<Utc>> {
        Ok(entry.created)
    }

    /// Archive an entry
    fn archive(&self, id: &str) -> Result<()>;

    /// Unarchive an entry
    fn unarchive(&self, id: &str) -> Result<()>;

    /// List all archived entries
    fn list_archived(&self) -> Result<Vec<Entry>>;

    /// List entries matching a specific branch (for worktree scoping)
    fn list_by_branch(&self, branch: &str) -> Result<Vec<Entry>>;

    /// List entries pending AI extraction
    fn list_pending(&self, limit: usize) -> Result<Vec<Entry>>;

    /// Mark an entry as extracted (no longer pending)
    fn mark_extracted(&self, id: &str) -> Result<()>;

    /// List entries in the in-context tier (always injected)
    fn list_pinned(&self) -> Result<Vec<Entry>>;

    /// List entries with positive feedback score, ordered by score descending
    fn list_helpful(&self, limit: usize) -> Result<Vec<Entry>>;

    /// List entries for a specific session
    fn list_by_session(&self, session_id: &str) -> Result<Vec<Entry>>;

    /// List learning entries that haven't been reviewed yet
    /// Used by the learning review hook to find entries needing analysis
    fn list_unreviewed_learnings(&self, limit: usize) -> Result<Vec<Entry>>;

    /// Mark an entry as reviewed (sets last_reviewed timestamp)
    fn mark_reviewed(&self, id: &str) -> Result<()>;

    /// List entries pending BM25/vector indexing (updated_at > indexed_at or indexed_at IS NULL)
    fn list_pending_index(&self, limit: usize) -> Result<Vec<Entry>>;

    /// Mark an entry as indexed (sets indexed_at to current timestamp)
    fn mark_indexed(&self, id: &str) -> Result<()>;

    /// Mark multiple entries as indexed in a single transaction
    fn mark_indexed_batch(&self, ids: &[&str]) -> Result<()>;

    /// Get the .cas directory path
    fn cas_dir(&self) -> &std::path::Path;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// Trait for rule storage operations
pub trait RuleStore: Send + Sync {
    /// Initialize the store
    fn init(&self) -> Result<()>;

    /// Generate a new unique rule ID
    fn generate_id(&self) -> Result<String>;

    /// Add a new rule
    fn add(&self, rule: &Rule) -> Result<()>;

    /// Get a rule by ID
    fn get(&self, id: &str) -> Result<Rule>;

    /// Update an existing rule
    fn update(&self, rule: &Rule) -> Result<()>;

    /// Delete a rule
    fn delete(&self, id: &str) -> Result<()>;

    /// List all rules
    fn list(&self) -> Result<Vec<Rule>>;

    /// List only proven rules (status = 'proven')
    fn list_proven(&self) -> Result<Vec<Rule>>;

    /// List critical rules (priority = 0, proven or draft)
    fn list_critical(&self) -> Result<Vec<Rule>>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// Trait for task storage operations
pub trait TaskStore: Send + Sync {
    /// Initialize the store (create tables, etc.)
    fn init(&self) -> Result<()>;

    /// Generate a new unique task ID (e.g., cas-a1b2)
    fn generate_id(&self) -> Result<String>;

    /// Add a new task
    fn add(&self, task: &Task) -> Result<()>;

    /// Atomically create a task and its initial dependencies.
    ///
    /// The default implementation is non-transactional and exists for compatibility with
    /// wrapper stores. Concrete stores with transactional support should override this.
    fn create_atomic(
        &self,
        task: &Task,
        blocked_by: &[String],
        epic_id: Option<&str>,
        created_by: Option<&str>,
    ) -> Result<()> {
        let now = chrono::Utc::now();

        if let Some(epic_id) = epic_id.filter(|id| !id.trim().is_empty()) {
            let epic_task = self.get(epic_id)?;
            if epic_task.task_type != TaskType::Epic {
                return Err(StoreError::Parse(format!(
                    "Task {} is not an epic (type: {})",
                    epic_id, epic_task.task_type
                )));
            }
        }

        self.add(task)?;

        for blocker_id in blocked_by
            .iter()
            .map(|id| id.trim())
            .filter(|id| !id.is_empty())
        {
            self.add_dependency(&Dependency {
                from_id: task.id.clone(),
                to_id: blocker_id.to_string(),
                dep_type: DependencyType::Blocks,
                created_at: now,
                created_by: created_by.map(ToString::to_string),
            })?;
        }

        if let Some(epic_id) = epic_id.filter(|id| !id.trim().is_empty()) {
            self.add_dependency(&Dependency {
                from_id: task.id.clone(),
                to_id: epic_id.to_string(),
                dep_type: DependencyType::ParentChild,
                created_at: now,
                created_by: created_by.map(ToString::to_string),
            })?;
        }

        Ok(())
    }

    /// Get a task by ID
    fn get(&self, id: &str) -> Result<Task>;

    /// Update an existing task.
    ///
    /// `updated_at` is **store-owned**: the store stamps it from its own clock
    /// and the caller's `task.updated_at` is ignored on this path. The stamp
    /// that was actually persisted is returned so callers never have to guess
    /// it with a second clock read.
    ///
    /// cas-ec74: deriving a lifecycle-notification occurrence from
    /// `task.updated_at` after calling `update()` compares two different clock
    /// reads and can never match the stored row. Derive it from this return
    /// value instead (see `supervisor_push::occurrence_from_updated_at`).
    ///
    /// The one sanctioned exception is
    /// `SqliteTaskStore::reopen_exact_with_conn`, where a caller-supplied
    /// timestamp is an explicit optimistic-concurrency key rather than a
    /// general write path.
    fn update(&self, task: &Task) -> Result<DateTime<Utc>>;

    /// Delete a task
    fn delete(&self, id: &str) -> Result<()>;

    /// List tasks with optional status filter
    fn list(&self, status: Option<TaskStatus>) -> Result<Vec<Task>>;

    /// List tasks that are ready to work on (open, not blocked)
    fn list_ready(&self) -> Result<Vec<Task>>;

    /// List blocked tasks with their blockers
    fn list_blocked(&self) -> Result<Vec<(Task, Vec<Task>)>>;

    /// List tasks with pending_verification=true (for PreToolUse jail check)
    fn list_pending_verification(&self) -> Result<Vec<Task>>;

    /// List tasks with pending_worktree_merge=true (for PreToolUse merge jail check)
    fn list_pending_worktree_merge(&self) -> Result<Vec<Task>>;

    /// Close the store
    fn close(&self) -> Result<()>;

    // Dependency operations

    /// Add a dependency between two items
    fn add_dependency(&self, dep: &Dependency) -> Result<()>;

    /// Remove a dependency
    fn remove_dependency(&self, from_id: &str, to_id: &str) -> Result<()>;

    /// Remove a dependency of a specific type (cas-6009).
    ///
    /// Returns `Ok(true)` if a dep of that type was found and deleted.
    /// Returns `Ok(false)` if no dep of that type exists between the two tasks.
    /// Leaves any deps of OTHER types between the same pair untouched.
    fn remove_dependency_of_type(
        &self,
        from_id: &str,
        to_id: &str,
        dep_type: DependencyType,
    ) -> Result<bool>;

    /// Get dependencies of a task (what it depends on)
    fn get_dependencies(&self, task_id: &str) -> Result<Vec<Dependency>>;

    /// Get dependents of a task (what depends on it)
    fn get_dependents(&self, task_id: &str) -> Result<Vec<Dependency>>;

    /// Get blocking dependencies (only type=blocks)
    fn get_blockers(&self, task_id: &str) -> Result<Vec<Task>>;

    /// Check if adding a dependency would create a cycle
    fn would_create_cycle(&self, from_id: &str, to_id: &str) -> Result<bool>;

    /// Get all dependencies in the system
    fn list_dependencies(&self, dep_type: Option<DependencyType>) -> Result<Vec<Dependency>>;

    /// Get all subtasks of a parent task (via Parent dependency)
    fn get_subtasks(&self, parent_id: &str) -> Result<Vec<Task>>;

    /// Get notes from sibling tasks (other subtasks of the same epic)
    /// Returns Vec<(task_id, title, notes)> for tasks with non-empty notes
    fn get_sibling_notes(
        &self,
        epic_id: &str,
        exclude_task_id: &str,
    ) -> Result<Vec<(String, String, String)>>;

    /// Get parent epic for a task (if it's a subtask)
    fn get_parent_epic(&self, task_id: &str) -> Result<Option<Task>>;
}

/// Trait for skill storage operations
pub trait SkillStore: Send + Sync {
    /// Initialize the store (create tables, etc.)
    fn init(&self) -> Result<()>;

    /// Generate a new unique skill ID (e.g., cas-sk01)
    fn generate_id(&self) -> Result<String>;

    /// Add a new skill
    fn add(&self, skill: &Skill) -> Result<()>;

    /// Get a skill by ID
    fn get(&self, id: &str) -> Result<Skill>;

    /// Update an existing skill
    fn update(&self, skill: &Skill) -> Result<()>;

    /// Delete a skill
    fn delete(&self, id: &str) -> Result<()>;

    /// List skills with optional status filter
    fn list(&self, status: Option<SkillStatus>) -> Result<Vec<Skill>>;

    /// List enabled skills
    fn list_enabled(&self) -> Result<Vec<Skill>>;

    /// Search skills by name or description
    fn search(&self, query: &str) -> Result<Vec<Skill>>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// Trait for entity (knowledge graph) storage operations
pub trait EntityStore: Send + Sync {
    /// Initialize the store (create tables, etc.)
    fn init(&self) -> Result<()>;

    // Entity operations

    /// Generate a new unique entity ID
    fn generate_entity_id(&self) -> Result<String>;

    /// Add a new entity
    fn add_entity(&self, entity: &Entity) -> Result<()>;

    /// Get an entity by ID
    fn get_entity(&self, id: &str) -> Result<Entity>;

    /// Get an entity by name (case-insensitive, including aliases)
    fn get_entity_by_name(
        &self,
        name: &str,
        entity_type: Option<EntityType>,
    ) -> Result<Option<Entity>>;

    /// Update an existing entity
    fn update_entity(&self, entity: &Entity) -> Result<()>;

    /// Delete an entity
    fn delete_entity(&self, id: &str) -> Result<()>;

    /// List entities with optional type filter
    fn list_entities(&self, entity_type: Option<EntityType>) -> Result<Vec<Entity>>;

    /// Search entities by name (substring match)
    fn search_entities(&self, query: &str, entity_type: Option<EntityType>) -> Result<Vec<Entity>>;

    // Relationship operations

    /// Generate a new unique relationship ID
    fn generate_relationship_id(&self) -> Result<String>;

    /// Add a new relationship
    fn add_relationship(&self, relationship: &Relationship) -> Result<()>;

    /// Get a relationship by ID
    fn get_relationship(&self, id: &str) -> Result<Relationship>;

    /// Get relationship between two entities (if exists)
    fn get_relationship_between(
        &self,
        source_id: &str,
        target_id: &str,
        relation_type: Option<RelationType>,
    ) -> Result<Option<Relationship>>;

    /// Update an existing relationship
    fn update_relationship(&self, relationship: &Relationship) -> Result<()>;

    /// Delete a relationship
    fn delete_relationship(&self, id: &str) -> Result<()>;

    /// List relationships with optional type filter
    fn list_relationships(&self, relation_type: Option<RelationType>) -> Result<Vec<Relationship>>;

    /// Get all relationships for an entity (as source or target)
    fn get_entity_relationships(&self, entity_id: &str) -> Result<Vec<Relationship>>;

    /// Get outgoing relationships (entity as source)
    fn get_outgoing_relationships(&self, entity_id: &str) -> Result<Vec<Relationship>>;

    /// Get incoming relationships (entity as target)
    fn get_incoming_relationships(&self, entity_id: &str) -> Result<Vec<Relationship>>;

    // Entity mention operations

    /// Add an entity mention (link entity to entry)
    fn add_mention(&self, mention: &EntityMention) -> Result<()>;

    /// Get mentions for an entity
    fn get_entity_mentions(&self, entity_id: &str) -> Result<Vec<EntityMention>>;

    /// Get mentions in an entry
    fn get_entry_mentions(&self, entry_id: &str) -> Result<Vec<EntityMention>>;

    /// Delete mentions for an entry (when entry is deleted/updated)
    fn delete_entry_mentions(&self, entry_id: &str) -> Result<()>;

    // Graph queries

    /// Get entities connected to a given entity (1-hop neighbors)
    fn get_connected_entities(&self, entity_id: &str) -> Result<Vec<(Entity, Relationship)>>;

    /// Get entries that mention an entity
    fn get_entity_entries(&self, entity_id: &str, limit: usize) -> Result<Vec<String>>;

    /// Close the store
    fn close(&self) -> Result<()>;
}
