//! Migration: recording_agents_idx_recording

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 178,
    name: "recording_agents_idx_recording",
    subsystem: Subsystem::Recordings,
    description: "Add index on recording_agents.recording_id for join lookups",
    up: &[
        "CREATE INDEX IF NOT EXISTS idx_recording_agents_recording ON recording_agents(recording_id)",
    ],
    // Retired by m289, which drops the recording tables: always detected, so an
    // upgraded store never recreates it and a fresh store never builds it.
    detect: Some(super::m289_drop_recording_index::RETIRED_DETECT),
};
