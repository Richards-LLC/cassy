//! Persist finding reach without changing the meaning of existing issue text.
use crate::migration::{Migration, Subsystem};

pub const MIGRATION: Migration = Migration {
    id: 296,
    name: "verification_issue_triage",
    subsystem: Subsystem::Verification,
    description: "Persist verification finding severity, reach, journey and reproduction",
    up: &[cas_store::VERIFICATION_ISSUE_TRIAGE_STATEMENT],
    detect: Some("SELECT EXISTS (SELECT 1 FROM pragma_table_info('verification_issues') WHERE name = 'triage')"),
};

#[cfg(test)]
mod tests {
    use cas_store::VerificationStore;

    #[test]
    fn legacy_issue_repair_is_theoretical_and_migration_is_detected() {
        crate::test_support::TestEnvGuard::run_with_temp_home(|home| {
            let project = home.join("project");
            std::fs::create_dir_all(&project).unwrap();
            let root = crate::store::init_cas_dir(&project).unwrap();
            let store = cas_store::SqliteVerificationStore::open(&root).unwrap();
            let old = cas_types::Verification::rejected("ver-old".into(), "cas-old".into(),
                "Legacy finding".into(), vec![cas_types::VerificationIssue::new("src/save.rs".into(),
                    "correctness".into(), "Code-read finding".into())]);
            store.add(&old).unwrap();
            let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
            conn.execute_batch("ALTER TABLE verification_issues DROP COLUMN triage; DELETE FROM cas_migrations WHERE id = 296;").unwrap();
            let repaired = cas_store::SqliteVerificationStore::open(&root).unwrap();
            let restored = repaired.get(&old.id).unwrap();
            assert_eq!(restored.issues[0].triage.reach, cas_types::FindingReach::Theoretical);
            assert_eq!(restored.issues[0].problem, old.issues[0].problem);
            crate::migration::check_migrations(&root).unwrap();
            assert_eq!(conn.query_row("SELECT applied_at FROM cas_migrations WHERE id=296", [],
                |row| row.get::<_, String>(0)).unwrap(), "DETECTED");
            cas_store::SqliteVerificationStore::open(&root).unwrap();
        });
    }
}
