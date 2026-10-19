use super::{Check, CheckStatus};
use std::path::Path;

pub(super) fn check(cas_dir: &Path) -> Check {
    match cas_store::communication_slo::last_session(cas_dir, chrono::Utc::now()) {
        Ok(Some(metrics)) => Check::new(
            "communication SLOs",
            if metrics.outcome() == "PASS" {
                CheckStatus::Ok
            } else {
                CheckStatus::Warning
            },
            metrics.message(),
        ),
        Ok(None) => Check::new(
            "communication SLOs",
            CheckStatus::Info,
            "UNKNOWN communication: no factory session recorded",
        ),
        Err(error) => Check::new(
            "communication SLOs",
            CheckStatus::Warning,
            format!(
                "UNKNOWN communication: cannot measure last session: {error}\nRun cas update --schema-only; then cas doctor"
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cas-884c: a schema the metrics query cannot prepare against yields an
    /// actionable, ASCII-only warning that names the missing column, never the
    /// metrics SQL or its comments.
    #[test]
    fn database_failure_names_the_cause_without_sql_and_stays_ascii_cas_884c() {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        use cas_store::PromptQueueStore;
        let queue = cas_store::SqlitePromptQueueStore::open(&root).unwrap();
        queue.init().unwrap();
        queue.enqueue_with_session("supervisor", "worker-a", "hello", "fixture-session").unwrap();
        let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        // The 24d3 QA database: a prompt_queue predating highest_stage.
        conn.execute_batch("ALTER TABLE prompt_queue RENAME COLUMN highest_stage TO stage_before_cas_447d").unwrap();
        let row = check(&root);
        assert!(matches!(row.status, CheckStatus::Warning), "{}", row.message);
        assert!(row.message.contains("no such column: highest_stage"), "{}", row.message);
        assert!(row.message.contains("cas update --schema-only"), "{}", row.message);
        assert!(!row.message.contains("cas-447d") && !row.message.contains("SELECT"), "{}", row.message);
        assert!(row.message.lines().all(|line| line.is_ascii()), "{}", row.message);
    }

    #[test]
    fn cas_7e24_doctor_outcome_values_targets_and_read_only() {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        assert!(matches!(check(&root).status, CheckStatus::Info));
        use cas_store::PromptQueueStore;
        let queue = cas_store::SqlitePromptQueueStore::open(&root).unwrap();
        queue.init().unwrap();
        let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        conn.execute_batch(include_str!(
            "../../../../scripts/fixtures/communication-slos/session.sql"
        ))
        .unwrap();
        cas_store::communication_slo::write_session_end(
            &root,
            "fixture-session",
            "2026-01-02T02:00:00Z".parse().unwrap(),
        )
        .unwrap();
        let row = check(&root);
        assert!(matches!(row.status, CheckStatus::Ok));
        assert!(row.message.starts_with("PASS"));
        assert_eq!(row.message.matches("target ").count(), 5);
        let rendered = super::super::render_report_plain(
            &[row],
            &[],
            &[],
            "fixture",
            "test",
            std::time::Duration::ZERO,
            false,
            80,
        );
        for name in [
            "false-abandon",
            "operator relay receipts",
            "duplicate body reads",
            "director nudges",
            "gated-without-outcome",
        ] {
            assert!(rendered.contains(name), "{rendered}");
        }
        assert!(rendered.lines().all(|line| line.chars().count() <= 80));
        conn.execute(
            "UPDATE prompt_queue SET highest_stage='abandoned' WHERE id=1",
            [],
        )
        .unwrap();
        cas_store::communication_slo::write_session_end(
            &root,
            "fixture-session",
            "2026-01-02T02:00:00Z".parse().unwrap(),
        )
        .unwrap();
        let row = check(&root);
        assert!(matches!(row.status, CheckStatus::Warning));
        assert!(row.message.starts_with("BREACH"));
        assert!(row.message.contains("BREACH false-abandon: 1 (target 0)"));
        conn.execute_batch("DROP TABLE communication_slo_summaries")
            .unwrap();
        let unavailable = check(&root);
        assert!(unavailable.message.starts_with("UNKNOWN"));
        assert!(unavailable.message.contains("cas update --schema-only"));
        assert_eq!(
            conn.query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name='communication_slo_summaries'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0,
            "doctor never repairs schema on read"
        );
    }
}
