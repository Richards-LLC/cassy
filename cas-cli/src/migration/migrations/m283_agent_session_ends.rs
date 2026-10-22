//! Preserve supervisor succession independently of disposable registry rows.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 283,
    name: "agent_session_ends",
    subsystem: Subsystem::Agents,
    description: "Retain session-end causes and atomically claimed supervisor lane snapshots",
    // Canonical additive baseline: safe on both lazily initialized and migrated stores.
    up: &[cas_store::AGENT_SCHEMA],
    detect: Some(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='agent_session_ends'",
    ),
};
