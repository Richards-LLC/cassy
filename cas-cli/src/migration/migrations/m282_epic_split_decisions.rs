//! Persist split expiry, deciding identities and unfinished terminal work.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 282,
    name: "epic_split_decisions",
    subsystem: Subsystem::Agents,
    description: "Record explicit split expiry, authenticated deciders and retryable cleanup",
    up: cas_store::EPIC_SPLIT_DECISION_SCHEMA_STATEMENTS,
    detect: Some("SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='epic_splits' AND sql LIKE '%''expired''%'"),
};
