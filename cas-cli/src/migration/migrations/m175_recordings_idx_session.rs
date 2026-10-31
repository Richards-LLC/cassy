//! Migration: recordings_idx_session

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 175,
    name: "recordings_idx_session",
    subsystem: Subsystem::Recordings,
    description: "Add index on recordings.session_id for session lookups",
    up: &["CREATE INDEX IF NOT EXISTS idx_recordings_session ON recordings(session_id)"],
    // Retired by m289, which drops the recording tables: always detected, so an
    // upgraded store never recreates it and a fresh store never builds it.
    detect: Some(super::m289_drop_recording_index::RETIRED_DETECT),
};
