//! Verifier observations held as rule candidates, outside `rules` (cas-f0a1).
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 280,
    name: "rule_candidates",
    subsystem: Subsystem::Rules,
    description: "Hold verifier observations as counted rule candidates that no agent context surfaces",
    up: cas_store::RULE_CANDIDATE_SCHEMA_STATEMENTS,
    detect: Some(
        "SELECT CASE WHEN
        EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='rule_candidates')
        AND EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='rule_candidate_occurrences')
        THEN 1 ELSE 0 END",
    ),
};
