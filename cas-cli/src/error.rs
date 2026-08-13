//! Error types for CAS
//!
//! Provides detailed error messages with suggestions for resolution.

// Dead code check enabled - all variants should be used

use thiserror::Error;

#[derive(Error, Debug)]
pub enum CasError {
    #[error("CAS not initialized")]
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

    #[error("entry already exists: {0}")]
    EntryExists(String),

    #[error("cyclic dependency would be created: {0} -> {1}")]
    CyclicDependency(String, String),

    #[error("entity not found: {0}")]
    EntityNotFound(String),

    #[error("relationship not found: {0}")]
    RelationshipNotFound(String),

    #[error("invalid entry type: {0}")]
    InvalidEntryType(String),

    #[error("invalid rule status: {0}")]
    InvalidRuleStatus(String),

    #[error("invalid task status: {0}")]
    InvalidTaskStatus(String),

    #[error("invalid rule category: {0}")]
    InvalidRuleCategory(String),

    #[error("invalid transaction state: {0}")]
    InvalidState(String),

    #[error("rollback failed: {0}")]
    RollbackFailed(String),

    #[error("similar entry exists: {id} (similarity: {similarity:.0}%)")]
    SimilarEntryExists { id: String, similarity: f64 },

    #[error("circular dependency would be created: {0} -> {1}")]
    CircularDependency(String, String),

    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("search error: {0}")]
    Search(#[from] tantivy::TantivyError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("yaml error: {0}")]
    Yaml(#[from] serde_yaml::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("type error: {0}")]
    Type(#[from] cas_types::TypeError),

    #[error("store error: {0}")]
    StoreErr(#[from] cas_store::StoreError),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("embedding error: {0}")]
    Embedding(String),

    #[error("vector store error: {0}")]
    VectorStore(String),

    #[error("model loading error: {0}")]
    ModelLoad(String),

    #[error("rerank error: {0}")]
    Rerank(String),

    #[error(
        "embedding dimension mismatch: stored dimension is {stored_dimension} (model: {stored_model}), but configured model '{configured_model}' has dimension {configured_dimension}"
    )]
    EmbeddingMigrationNeeded {
        stored_model: String,
        stored_dimension: usize,
        configured_model: String,
        configured_dimension: usize,
    },

    #[error("migration failed: {name} - {reason}")]
    MigrationFailed { name: String, reason: String },

    #[error(
        "binary {binary} requires schema v{required} — run `cas update --schema-only` \
         (current schema v{current}; pending migration {pending_migration})"
    )]
    SchemaOutdated {
        binary: String,
        current: u32,
        required: u32,
        pending_migration: String,
    },

    #[error("{0}")]
    Other(String),

    #[error("core error: {0}")]
    Core(#[from] cas_core::CoreError),
}

impl CasError {
    /// Get a helpful suggestion for how to resolve this error
    pub fn suggestion(&self) -> Option<&'static str> {
        match self {
            CasError::NotInitialized => Some(
                "Run 'cas init' to initialize CAS in this directory.\n\
                 This creates a .cas/ directory with the SQLite database.",
            ),
            CasError::EntryNotFound(_) => Some(
                "Use 'cas list' to see all entries, or 'cas search <query>' to find entries.\n\
                 Entry IDs are in the format 'cas-XXXX' (8 hex characters).",
            ),
            CasError::RuleNotFound(_) => Some(
                "Use 'cas rules list' to see all rules.\n\
                 Rule IDs are in the format 'rule-NNN'.",
            ),
            CasError::TaskNotFound(_) => Some(
                "Use 'cas task list' to see all tasks.\n\
                 Task IDs are in the format 'cas-XXXX' (8 hex characters).",
            ),
            CasError::SkillNotFound(_) => Some(
                "Use 'cas skill list' to see all skills.\n\
                 Skill IDs are in the format 'cas-skXX'.",
            ),
            CasError::EntityNotFound(_) => Some(
                "Use 'cas entity list' to see all entities.\n\
                 Entity IDs are in the format 'ent-XXXX'.",
            ),
            CasError::RelationshipNotFound(_) => Some(
                "Use 'cas entity relationships' to see all relationships.\n\
                 Relationship IDs are in the format 'rel-XXXX'.",
            ),
            CasError::InvalidEntryType(_) => Some(
                "Valid entry types: observation, decision, pattern, context, insight, error\n\
                 Use --type <type> to specify the entry type.",
            ),
            CasError::InvalidRuleStatus(_) => Some(
                "Valid rule statuses: draft, proven, stale, retired\n\
                 Rules are promoted to 'proven' when marked helpful.",
            ),
            CasError::InvalidTaskStatus(_) => Some(
                "Valid task statuses: open, in_progress, blocked, closed\n\
                 Use 'cas task start <id>' to set to in_progress.",
            ),
            CasError::InvalidRuleCategory(_) => Some(
                "Valid rule categories: general, convention, security, performance, architecture, error-handling\n\
                 Use --category <cat> to categorize rules for better filtering.",
            ),
            CasError::SimilarEntryExists { .. } => Some(
                "A similar entry already exists. Options:\n\
                 - Use '--force' to add anyway\n\
                 - Use 'cas show <id>' to view the existing entry\n\
                 - Use 'cas update <id>' to update the existing entry",
            ),
            CasError::CircularDependency(_, _) => Some(
                "Task dependencies cannot form cycles.\n\
                 Use 'cas task dep tree <id>' to visualize the dependency graph.",
            ),
            CasError::Database(_) => Some(
                "Database error occurred. Try:\n\
                 - Run 'cas doctor' to check database health\n\
                 - Check disk space and permissions on .cas/ directory",
            ),
            CasError::Search(_) => Some(
                "Search index error. Try:\n\
                 - Run 'cas reindex --bm25' to rebuild the search index",
            ),
            CasError::Embedding(_) | CasError::ModelLoad(_) => Some(
                "Embedding model error. Try:\n\
                 - Use '--no-semantic' for BM25-only search\n\
                 - Check disk space for model cache (~1GB)\n\
                 - Models are cached in .cas/models/",
            ),
            CasError::VectorStore(_) => Some(
                "Vector store error. Try:\n\
                 - Check .cas/vectors.hnsw file permissions\n\
                 - Delete vectors.hnsw to reset the vector store",
            ),
            CasError::EmbeddingMigrationNeeded { .. } => Some(
                "The embedding model dimension has changed.\n\
                 Delete vectors.hnsw and vectors.meta.json files to reset.",
            ),
            CasError::Parse(_) => Some(
                "Could not parse the input. Check the format and try again.\n\
                 Use '--help' to see valid options for the command.",
            ),
            CasError::MigrationFailed { .. } => Some(
                "A database migration failed. Options:\n\
                 - Check disk space and permissions on .cas/ directory\n\
                 - Run 'cas doctor' to diagnose issues\n\
                 - Restore from backup if needed",
            ),
            CasError::SchemaOutdated { .. } => Some(
                "The database schema needs to be updated.\n\
                 Run 'cas update --schema-only' to apply pending migrations.\n\
                 Use 'cas update --check' to preview changes first.",
            ),
            _ => None,
        }
    }

    /// Format the error with suggestion for display
    pub fn with_suggestion(&self) -> String {
        let mut msg = format!("Error: {self}");
        if let Some(suggestion) = self.suggestion() {
            msg.push_str("\n\nSuggestion:\n");
            msg.push_str(suggestion);
        }
        msg
    }
}

pub type Result<T> = std::result::Result<T, CasError>;

/// Backwards compatibility alias
pub type MemError = CasError;
