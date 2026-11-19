//! Error types for CAS store

use thiserror::Error;

#[derive(Error, Debug)]
pub enum StoreError {
    #[error("not initialized")]
    NotInitialized,

    #[error("not found: {0}")]
    NotFound(String),

    #[error("entry not found: {0}")]
    EntryNotFound(String),

    #[error("rule not found: {0}")]
    RuleNotFound(String),

    #[error("task not found: {0}")]
    TaskNotFound(String),

    #[error("skill not found: {0}")]
    SkillNotFound(String),

    #[error("entity not found: {0}")]
    EntityNotFound(String),

    #[error("relationship not found: {0}")]
    RelationshipNotFound(String),

    #[error("entry already exists: {0}")]
    EntryExists(String),

    #[error("cyclic dependency would be created: {0} -> {1}")]
    CyclicDependency(String, String),

    #[error("circular dependency would be created: {0} -> {1}")]
    CircularDependency(String, String),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("parse error: {0}")]
    Parse(String),

    /// Another close already owns the exact active proof cycle. Preserve its
    /// transactional snapshot so callers can report it without a racy re-read.
    #[error(
        "verification dispatch {} is already active for this task with a different requester, owner or proof boundary",
        .dispatch.id
    )]
    VerificationDispatchConflict {
        dispatch: Box<cas_types::VerificationDispatch>,
    },

    #[error("premerge_repair_requires_new_commit: automatic repair for {dispatch_id} needs a new provable source commit")]
    PremergeRepairRequiresNewCommit { dispatch_id: String },

    /// A verifier handoff was refused because its dispatch is not the exact
    /// pending dispatch owned by the issuer. Not a parse failure.
    #[error("verifier_handoff_not_active: dispatch {dispatch_id} is not the exact pending dispatch owned by this issuer")]
    VerifierHandoffNotActive { dispatch_id: String },

    /// Another verifier handoff for the same parent is still waiting to bind.
    /// It clears on its own, so a launch queues behind it instead of failing.
    #[error("another task-verifier spawn is already awaiting SubagentStart for this parent; wait for it to bind or for exact failure cleanup/expiry")]
    VerifierHandoffPending,

    #[error(
        "local {entity_type} {operation} committed, but cloud sync is degraded; durable retry intent retained for {entity_id}: {reason}"
    )]
    SyncDegradedAfterCommit {
        entity_type: String,
        entity_id: String,
        operation: String,
        reason: String,
    },

    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;
