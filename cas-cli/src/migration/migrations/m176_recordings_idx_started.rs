//! Migration: recordings_idx_started

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 176,
    name: "recordings_idx_started",
    subsystem: Subsystem::Recordings,
    description: "Add index on recordings.started_at for date range queries",
    up: &["CREATE INDEX IF NOT EXISTS idx_recordings_started ON recordings(started_at DESC)"],
    // Retired by m289, which drops the recording tables: always detected, so an
    // upgraded store never recreates it and a fresh store never builds it.
    detect: Some(super::m289_drop_recording_index::RETIRED_DETECT),
};
