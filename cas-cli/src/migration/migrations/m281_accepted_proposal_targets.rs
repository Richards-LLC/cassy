//! Local record of proposals accepted into this project (cas-3556).
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 281,
    name: "accepted_proposal_targets",
    subsystem: Subsystem::Tasks,
    description: "Record accepted cross-project proposals so ownership never trusts task notes",
    up: cas_store::ACCEPTED_PROPOSAL_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='accepted_proposal_targets') THEN 1 ELSE 0 END",
    ),
};
