//! Keep verification rows readable by older binaries during a rolling update.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 291,
    name: "verification_status_detail",
    subsystem: Subsystem::Verification,
    description: "Store extended verification semantics beside a legacy-compatible status",
    up: &[
        cas_store::VERIFICATION_STATUS_DETAIL_STATEMENT,
        cas_store::VERIFICATION_STATUS_NORMALIZE_STATEMENT,
    ],
    detect: Some(
        "SELECT EXISTS (SELECT 1 FROM pragma_table_info('verifications') WHERE name = 'status_detail')",
    ),
};
