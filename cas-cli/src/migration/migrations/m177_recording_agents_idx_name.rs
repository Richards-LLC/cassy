//! Migration: recording_agents_idx_name

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 177,
    name: "recording_agents_idx_name",
    subsystem: Subsystem::Recordings,
    description: "Add index on recording_agents.agent_name for agent lookups",
    up: &["CREATE INDEX IF NOT EXISTS idx_recording_agents_name ON recording_agents(agent_name)"],
    // Retired by m289, which drops the recording tables: always detected, so an
    // upgraded store never recreates it and a fresh store never builds it.
    detect: Some(super::m289_drop_recording_index::RETIRED_DETECT),
};
