//! Error types for CAS core business logic
//!
//! This module provides a unified error type that wraps errors from all CAS
//! subsystems (store, embedding, types) and adds core-specific error variants.
//!
//! # Example
//!
//! ```rust,ignore
//! use cas_core::{CoreError, Result};
//!
//! fn example() -> Result<()> {
//!     if some_condition {
//!         return Err(CoreError::NotFound("resource".to_string()));
//!     }
//!     Ok(())
//! }
//! ```

use thiserror::Error;

/// Core error type that unifies errors from all CAS subsystems
#[derive(Error, Debug)]
pub enum CoreError {
    /// CAS has not been initialized in this directory
    #[error("CAS not initialized. Run 'cas init' first.")]
    NotInitialized,

    /// A requested resource was not found
    #[error("not found: {0}")]
    NotFound(String),

    /// Invalid configuration
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    /// Parse error (YAML, JSON, etc.)
    #[error("parse error: {0}")]
    Parse(String),

    /// I/O error
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// Generic error for other cases
    #[error("{0}")]
    Other(String),

    /// Error from the store layer
    #[error("store error: {0}")]
    Store(#[from] cas_store::StoreError),

    /// Error from type parsing
    #[error("type error: {0}")]
    Type(#[from] cas_types::TypeError),

    /// Database error
    #[error("database error: {0}")]
    Database(#[from] rusqlite::Error),

    /// Migration failed
    #[error("migration failed: {name} - {reason}")]
    MigrationFailed { name: String, reason: String },

    /// Schema is outdated
    #[error("database schema outdated: v{current} -> v{required}. Run 'cas update --schema-only'")]
    SchemaOutdated { current: u32, required: u32 },
}

/// Result type alias using CoreError
pub type Result<T> = std::result::Result<T, CoreError>;

#[cfg(test)]
mod tests {
    use crate::error::*;


    #[test]
    fn test_not_found_error() {
        let task_id = "task-123";
        let err = CoreError::NotFound(task_id.to_string());

        assert!(err.to_string().contains(task_id));
    }

    #[test]
    fn test_invalid_config_error() {
        let supplied = "missing field";
        let err = CoreError::InvalidConfig(supplied.to_string());
        assert!(err.to_string().contains(supplied));
    }

    #[test]
    fn test_parse_error() {
        let supplied = "unexpected token";
        let err = CoreError::Parse(supplied.to_string());
        assert!(err.to_string().contains(supplied));
    }

    #[test]
    fn test_io_error_conversion() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
        let core_err: CoreError = io_err.into();
        assert!(matches!(core_err, CoreError::Io(_)));
    }

    #[test]
    fn test_other_error() {
        let supplied = "something went wrong";
        let err = CoreError::Other(supplied.to_string());
        assert!(err.to_string().contains(supplied));
    }
}
