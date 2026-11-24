//! Record on a verification dispatch what its review covers (GH #2417).
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 302,
    name: "verification_dispatch_reviewed_proof",
    subsystem: Subsystem::Verification,
    description: "Record the proof reference and artifact digests a verification dispatch reviews",
    up: &[cas_store::VERIFICATION_DISPATCH_REVIEWED_PROOF_STATEMENT],
    detect: Some("SELECT EXISTS (SELECT 1 FROM pragma_table_info('verification_dispatches') WHERE name = 'reviewed_proof')"),
};
