//! Migration: recording_events_idx_timestamp

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 180,
    name: "recording_events_idx_timestamp",
    subsystem: Subsystem::Recordings,
    description: "Add index on recording_events.timestamp_ms for time-based queries",
    up: &[
        "CREATE INDEX IF NOT EXISTS idx_recording_events_timestamp ON recording_events(timestamp_ms)",
    ],
    // Retired by m289, which drops the recording tables: always detected, so an
    // upgraded store never recreates it and a fresh store never builds it.
    detect: Some(super::m289_drop_recording_index::RETIRED_DETECT),
};
