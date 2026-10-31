//! Migration: recordings_fts_create

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 174,
    name: "recordings_fts_create",
    subsystem: Subsystem::Recordings,
    description: "Create FTS5 virtual table for searchable recording content",
    up: &[
        "CREATE VIRTUAL TABLE IF NOT EXISTS recordings_fts USING fts5(
            recording_id UNINDEXED,
            content,
            content_type UNINDEXED,
            timestamp_ms UNINDEXED
        )",
    ],
    // Retired by m289, which drops the recording tables: always detected, so an
    // upgraded store never recreates it and a fresh store never builds it.
    detect: Some(super::m289_drop_recording_index::RETIRED_DETECT),
};
