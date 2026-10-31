//! Migration: recording_events_create_table

use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 173,
    name: "recording_events_create_table",
    subsystem: Subsystem::Recordings,
    description: "Create recording_events table for Cassy entity correlation",
    up: &["CREATE TABLE IF NOT EXISTS recording_events (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            recording_id TEXT NOT NULL,
            timestamp_ms INTEGER NOT NULL,
            event_type TEXT NOT NULL,
            entity_type TEXT,
            entity_id TEXT,
            metadata TEXT,
            FOREIGN KEY (recording_id) REFERENCES recordings(id) ON DELETE CASCADE
        )"],
    // Retired by m289, which drops the recording tables: always detected, so an
    // upgraded store never recreates it and a fresh store never builds it.
    detect: Some(super::m289_drop_recording_index::RETIRED_DETECT),
};
