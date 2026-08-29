//! Memory-specific business logic for CAS.
//!
//! Currently hosts the pre-insert overlap detection workflow shipped by
//! cas-4721. The workflow is defined in the salvaged skill reference at
//! `cas-cli/src/builtins/skills/cas-memory-management/references/overlap-detection.md`
//! and implemented here as a pure-Rust function with no MCP or store
//! dependency — callers fetch candidates and pass them in.

pub mod overlap;
pub mod hygiene;

pub use hygiene::{
    ContaminatedEntry, HIGH_IMPORTANCE_PREFERENCE_THRESHOLD, TOOL_CALL_ARTIFACT_PATTERNS,
    contamination_patterns, find_contaminated_entries,
};
pub use overlap::{
    CandidateFacets, DimensionScores, NewMemoryFacets, OverlapDecision, OverlapMatch,
    OverlapRecommendation, check_overlap, extract_facets_from_body,
};
