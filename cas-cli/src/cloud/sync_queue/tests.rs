use tempfile::TempDir;

use crate::cloud::sync_queue::{EntityType, SyncOperation, SyncQueue};

fn create_test_queue() -> (TempDir, SyncQueue) {
    let temp = TempDir::new().unwrap();
    let queue = SyncQueue::open(temp.path()).unwrap();
    queue.init().unwrap();
    (temp, queue)
}

#[test]
fn test_enqueue_and_pending() {
    let (_temp, queue) = create_test_queue();

    queue
        .enqueue(
            EntityType::Entry,
            "entry-1",
            SyncOperation::Upsert,
            Some(r#"{"id":"entry-1"}"#),
        )
        .unwrap();

    queue
        .enqueue(
            EntityType::Task,
            "task-1",
            SyncOperation::Upsert,
            Some(r#"{"id":"task-1"}"#),
        )
        .unwrap();

    let pending = queue.pending(10, 5).unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].entity_id, "entry-1");
    assert_eq!(pending[1].entity_id, "task-1");
}

#[test]
fn test_coalesce_updates() {
    let (_temp, queue) = create_test_queue();

    queue
        .enqueue(
            EntityType::Entry,
            "entry-1",
            SyncOperation::Upsert,
            Some(r#"{"content":"v1"}"#),
        )
        .unwrap();

    queue
        .enqueue(
            EntityType::Entry,
            "entry-1",
            SyncOperation::Upsert,
            Some(r#"{"content":"v2"}"#),
        )
        .unwrap();

    let pending = queue.pending(10, 5).unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].payload.as_ref().unwrap().contains("v2"));
}

#[test]
fn test_mark_synced() {
    let (_temp, queue) = create_test_queue();

    queue
        .enqueue(EntityType::Entry, "entry-1", SyncOperation::Upsert, None)
        .unwrap();

    let pending = queue.pending(10, 5).unwrap();
    assert_eq!(pending.len(), 1);

    queue.mark_synced(pending[0].id).unwrap();

    let pending = queue.pending(10, 5).unwrap();
    assert_eq!(pending.len(), 0);
}

#[test]
fn test_mark_failed_and_retry_limit() {
    let (_temp, queue) = create_test_queue();

    queue
        .enqueue(EntityType::Entry, "entry-1", SyncOperation::Upsert, None)
        .unwrap();

    let pending = queue.pending(10, 3).unwrap();
    let id = pending[0].id;

    for i in 0..3 {
        queue.mark_failed(id, &format!("Error {i}")).unwrap();
    }

    let pending = queue.pending(10, 3).unwrap();
    assert_eq!(pending.len(), 0);

    assert_eq!(queue.queue_depth().unwrap(), 1);
}

#[test]
fn diagnostic_keeps_a_server_skipped_row_retryable() {
    let (_temp, queue) = create_test_queue();
    queue
        .enqueue(
            EntityType::Task,
            "task-skipped",
            SyncOperation::Upsert,
            None,
        )
        .unwrap();

    let queued = queue.pending(10, 5).unwrap();
    queue
        .record_diagnostic(
            queued[0].id,
            "cloud skipped task due to project-scoped identity collision",
        )
        .unwrap();

    let after = queue.pending(10, 5).unwrap();
    assert_eq!(after.len(), 1, "diagnostics must not dequeue genuine work");
    assert_eq!(after[0].retry_count, 0, "a skip is not a transport retry");
    assert_eq!(
        after[0].last_error.as_deref(),
        Some("cloud skipped task due to project-scoped identity collision")
    );
}

#[test]
fn conflict_journal_retains_the_discarded_row_and_prunes_by_age() {
    let (_temp, queue) = create_test_queue();

    queue
        .record_conflict(
            "task",
            "cas-conflict",
            r#"{\"id\":\"cas-conflict\",\"notes\":\"local note\"}"#,
            "remote",
            "timestamp_lww",
            None,
            None,
        )
        .unwrap();

    let conflicts = queue.list_conflicts(10).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].entity_type, "task");
    assert_eq!(conflicts[0].entity_id, "cas-conflict");
    assert_eq!(
        conflicts[0].discarded_row_json,
        r#"{\"id\":\"cas-conflict\",\"notes\":\"local note\"}"#
    );
    assert_eq!(conflicts[0].winner_side, "remote");
    assert_eq!(conflicts[0].strategy, "timestamp_lww");
    assert_eq!(queue.unreviewed_conflict_count().unwrap(), 1);
    assert_eq!(queue.prune_conflicts(0).unwrap(), 1);
    assert!(queue.list_conflicts(10).unwrap().is_empty());
}

#[test]
fn health_reports_pending_age_and_last_push_error() {
    let (_temp, queue) = create_test_queue();
    queue
        .enqueue(
            EntityType::Entry,
            "entry-health",
            SyncOperation::Upsert,
            None,
        )
        .unwrap();
    let id = queue.pending(10, 5).unwrap()[0].id;
    queue.mark_failed(id, "Network error: offline").unwrap();

    let health = queue
        .health(5, chrono::Utc::now() + chrono::Duration::hours(7))
        .unwrap();
    assert_eq!(health.pending, 1);
    assert!(health.oldest_age_secs.unwrap() >= 7 * 60 * 60 - 1);
    assert_eq!(health.last_error.as_deref(), Some("Network error: offline"));
}

#[test]
fn test_metadata() {
    let (_temp, queue) = create_test_queue();

    assert!(queue.get_metadata("last_push").unwrap().is_none());

    queue
        .set_metadata("last_push", "2024-01-01T00:00:00Z")
        .unwrap();
    assert_eq!(
        queue.get_metadata("last_push").unwrap(),
        Some("2024-01-01T00:00:00Z".to_string())
    );

    queue
        .set_metadata("last_push", "2024-01-02T00:00:00Z")
        .unwrap();
    assert_eq!(
        queue.get_metadata("last_push").unwrap(),
        Some("2024-01-02T00:00:00Z".to_string())
    );

    queue.delete_metadata("last_push").unwrap();
    assert!(queue.get_metadata("last_push").unwrap().is_none());
}

#[test]
fn delete_metadata_with_prefix_removes_only_matching_watermarks() {
    let (_temp, queue) = create_test_queue();

    queue
        .set_metadata("last_team_pull_at_team-a_project-a", "2024-01-01T00:00:00Z")
        .unwrap();
    queue
        .set_metadata("last_team_pull_at_team-b_project-b", "2024-01-02T00:00:00Z")
        .unwrap();
    queue
        .set_metadata("last_team_pull_at%team-c", "should-not-match")
        .unwrap();
    queue
        .set_metadata("last_pull_at", "2024-01-03T00:00:00Z")
        .unwrap();

    assert_eq!(
        queue
            .delete_metadata_with_prefix("last_team_pull_at_")
            .unwrap(),
        2
    );
    assert!(queue
        .get_metadata("last_team_pull_at_team-a_project-a")
        .unwrap()
        .is_none());
    assert!(queue
        .get_metadata("last_team_pull_at_team-b_project-b")
        .unwrap()
        .is_none());
    assert_eq!(
        queue.get_metadata("last_team_pull_at%team-c").unwrap(),
        Some("should-not-match".to_string())
    );
    assert!(queue.get_metadata("last_pull_at").unwrap().is_some());
}

#[test]
fn test_pending_by_type() {
    let (_temp, queue) = create_test_queue();

    queue
        .enqueue(EntityType::Entry, "e1", SyncOperation::Upsert, None)
        .unwrap();
    queue
        .enqueue(EntityType::Entry, "e2", SyncOperation::Upsert, None)
        .unwrap();
    queue
        .enqueue(EntityType::Task, "t1", SyncOperation::Upsert, None)
        .unwrap();
    queue
        .enqueue(
            EntityType::TaskDependency,
            "t1:t2:blocks",
            SyncOperation::Upsert,
            Some(r#"{"from_id":"t1","to_id":"t2","dep_type":"blocks"}"#),
        )
        .unwrap();
    queue
        .enqueue(EntityType::Rule, "r1", SyncOperation::Delete, None)
        .unwrap();

    let by_type = queue.pending_by_type(10, 5).unwrap();
    assert_eq!(by_type.entries.len(), 2);
    assert_eq!(by_type.tasks.len(), 1);
    assert_eq!(by_type.task_dependencies.len(), 1);
    assert_eq!(by_type.rules.len(), 1);
    assert_eq!(by_type.skills.len(), 0);
    assert_eq!(by_type.total(), 5);
}

#[test]
fn test_delete_operation() {
    let (_temp, queue) = create_test_queue();

    queue
        .enqueue(
            EntityType::Entry,
            "entry-1",
            SyncOperation::Upsert,
            Some(r#"{"id":"entry-1"}"#),
        )
        .unwrap();

    queue
        .enqueue(EntityType::Entry, "entry-1", SyncOperation::Delete, None)
        .unwrap();

    let pending = queue.pending(10, 5).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].operation, SyncOperation::Delete);
    assert!(pending[0].payload.is_none());
}

#[test]
fn test_team_id_enqueue_and_pending() {
    let (_temp, queue) = create_test_queue();

    queue
        .enqueue(EntityType::Entry, "entry-1", SyncOperation::Upsert, None)
        .unwrap();

    queue
        .enqueue_for_team(
            EntityType::Entry,
            "entry-2",
            SyncOperation::Upsert,
            Some(r#"{"id":"entry-2"}"#),
            "team-123",
        )
        .unwrap();

    let personal = queue.pending(10, 5).unwrap();
    assert_eq!(personal.len(), 1);
    assert_eq!(personal[0].entity_id, "entry-1");
    assert!(personal[0].team_id.is_none());

    let team = queue.pending_for_team("team-123", 10, 5).unwrap();
    assert_eq!(team.len(), 1);
    assert_eq!(team[0].entity_id, "entry-2");
    assert_eq!(team[0].team_id, Some("team-123".to_string()));
}

#[test]
fn test_team_id_isolation() {
    let (_temp, queue) = create_test_queue();

    queue
        .enqueue(EntityType::Entry, "entry-1", SyncOperation::Upsert, None)
        .unwrap();
    queue
        .enqueue_for_team(
            EntityType::Entry,
            "entry-1",
            SyncOperation::Upsert,
            None,
            "team-a",
        )
        .unwrap();
    queue
        .enqueue_for_team(
            EntityType::Entry,
            "entry-1",
            SyncOperation::Upsert,
            None,
            "team-b",
        )
        .unwrap();

    let all = queue.list_all(10).unwrap();
    assert_eq!(all.len(), 3);

    assert_eq!(queue.pending(10, 5).unwrap().len(), 1);
    assert_eq!(queue.pending_for_team("team-a", 10, 5).unwrap().len(), 1);
    assert_eq!(queue.pending_for_team("team-b", 10, 5).unwrap().len(), 1);
}

#[test]
fn test_drain_by_team() {
    let (_temp, queue) = create_test_queue();

    queue
        .enqueue_for_team(
            EntityType::Entry,
            "e1",
            SyncOperation::Upsert,
            None,
            "team-a",
        )
        .unwrap();
    queue
        .enqueue_for_team(
            EntityType::Task,
            "t1",
            SyncOperation::Upsert,
            None,
            "team-a",
        )
        .unwrap();

    queue
        .enqueue_for_team(
            EntityType::Entry,
            "e2",
            SyncOperation::Upsert,
            None,
            "team-b",
        )
        .unwrap();

    let drained = queue.drain_by_team("team-a", 5).unwrap();
    assert_eq!(drained.len(), 2);

    assert_eq!(queue.pending_for_team("team-a", 10, 5).unwrap().len(), 0);

    assert_eq!(queue.pending_for_team("team-b", 10, 5).unwrap().len(), 1);
}

#[test]
fn test_pending_count_for_team() {
    let (_temp, queue) = create_test_queue();

    queue
        .enqueue_for_team(
            EntityType::Entry,
            "e1",
            SyncOperation::Upsert,
            None,
            "team-123",
        )
        .unwrap();
    queue
        .enqueue_for_team(
            EntityType::Entry,
            "e2",
            SyncOperation::Upsert,
            None,
            "team-123",
        )
        .unwrap();
    queue
        .enqueue_for_team(
            EntityType::Entry,
            "e3",
            SyncOperation::Upsert,
            None,
            "other-team",
        )
        .unwrap();

    assert_eq!(queue.pending_count_for_team("team-123", 5).unwrap(), 2);
    assert_eq!(queue.pending_count_for_team("other-team", 5).unwrap(), 1);
    assert_eq!(queue.pending_count_for_team("nonexistent", 5).unwrap(), 0);
}

#[test]
fn test_pending_by_type_for_team() {
    let (_temp, queue) = create_test_queue();

    queue
        .enqueue_for_team(
            EntityType::Entry,
            "e1",
            SyncOperation::Upsert,
            None,
            "team-123",
        )
        .unwrap();
    queue
        .enqueue_for_team(
            EntityType::Task,
            "t1",
            SyncOperation::Upsert,
            None,
            "team-123",
        )
        .unwrap();
    queue
        .enqueue_for_team(
            EntityType::Task,
            "t2",
            SyncOperation::Upsert,
            None,
            "team-123",
        )
        .unwrap();

    let by_type = queue.pending_by_type_for_team("team-123", 10, 5).unwrap();
    assert_eq!(by_type.entries.len(), 1);
    assert_eq!(by_type.tasks.len(), 2);
    assert_eq!(by_type.rules.len(), 0);
    assert_eq!(by_type.skills.len(), 0);
}

// --- cas-8dd8 regression tests (defects B + C) ---

/// AC3: A single un-pushable queue item (null payload for upsert) must not
/// freeze the rest of the queue.  The fixed push_batch calls mark_failed
/// instead of silently skipping, so the poison accumulates retry_count until
/// it transitions from `pending` to `failed`.  Good items behind it remain
/// pending and oldest_item advances past the parked head.
#[test]
fn test_poison_head_doesnt_block_queue() {
    let (_temp, queue) = create_test_queue();
    const MAX_RETRIES: i32 = 5;

    // Enqueue the poison head first (null payload → invalid upsert).
    queue
        .enqueue(EntityType::Task, "task-poison", SyncOperation::Upsert, None)
        .unwrap();

    // Two healthy items enqueued after the poison.
    queue
        .enqueue(
            EntityType::Task,
            "task-good-1",
            SyncOperation::Upsert,
            Some(r#"{"id":"task-good-1"}"#),
        )
        .unwrap();
    queue
        .enqueue(
            EntityType::Task,
            "task-good-2",
            SyncOperation::Upsert,
            Some(r#"{"id":"task-good-2"}"#),
        )
        .unwrap();

    // Locate the poison item's id.
    let all_pending = queue.pending(10, MAX_RETRIES).unwrap();
    assert_eq!(all_pending.len(), 3);
    let poison_id = all_pending
        .iter()
        .find(|i| i.entity_id == "task-poison")
        .unwrap()
        .id;

    // Simulate the fixed push_batch calling mark_failed MAX_RETRIES times on
    // the poison.  Each call increments retry_count; once retry_count reaches
    // MAX_RETRIES the item stops appearing in pending() and is counted as
    // failed in stats().
    for attempt in 0..MAX_RETRIES {
        queue
            .mark_failed(
                poison_id,
                &format!("missing payload for upsert operation (attempt {attempt})"),
            )
            .unwrap();
    }

    // --- AC3 assertions ---

    // Good items must still be pending; poison must not appear.
    let still_pending = queue.pending(10, MAX_RETRIES).unwrap();
    assert_eq!(still_pending.len(), 2, "good items must remain pending");
    assert!(
        still_pending.iter().all(|i| i.entity_id != "task-poison"),
        "poison must not appear in pending after max_retries failures"
    );

    // Stats: 1 failed, 2 pending.
    let stats = queue.stats(MAX_RETRIES).unwrap();
    assert_eq!(stats.failed, 1, "poison must be counted as failed");
    assert_eq!(stats.pending, 2, "good items must be counted as pending");

    // oldest_item must advance past the parked poison and reflect a good item.
    // (Before the fix, oldest_item stayed frozen on the poison's created_at
    // because the stats query did not filter by retry_count.)
    assert!(
        stats.oldest_item.is_some(),
        "oldest_item must be Some — queue is not empty of pending items"
    );
}

/// Terminal rows preserve their server diagnostic when an operator explicitly
/// requeues them after the remote rejection has been repaired.
#[test]
fn test_retry_failed_requeues_without_erasing_diagnostic() {
    let (_temp, queue) = create_test_queue();
    const MAX_RETRIES: i32 = 5;
    queue
        .enqueue(
            EntityType::Task,
            "task-server-collision",
            SyncOperation::Upsert,
            Some(r#"{"id":"task-server-collision"}"#),
        )
        .unwrap();
    let id = queue.pending(10, MAX_RETRIES).unwrap()[0].id;
    for _ in 0..MAX_RETRIES {
        queue
            .mark_failed(id, r#"server response: {"tasks":{"skipped":1}}"#)
            .unwrap();
    }

    assert_eq!(queue.stats(MAX_RETRIES).unwrap().failed, 1);
    assert_eq!(queue.retry_failed(MAX_RETRIES).unwrap(), 1);
    let retried = queue.pending(10, MAX_RETRIES).unwrap();
    assert_eq!(retried.len(), 1);
    assert_eq!(retried[0].retry_count, 0);
    assert_eq!(
        retried[0].last_error.as_deref(),
        Some(r#"server response: {"tasks":{"skipped":1}}"#)
    );
}

/// GH #652: migration must repair duplicate rows created before the unique
/// identity index existed, retaining the newest payload for the next push.
#[test]
fn queue_migration_collapses_legacy_duplicate_personal_rows() {
    use rusqlite::Connection;

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("cas.db");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE sync_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                operation TEXT NOT NULL,
                payload TEXT,
                team_id TEXT,
                created_at TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            );
            INSERT INTO sync_queue
                (entity_type, entity_id, operation, payload, team_id, created_at)
            VALUES
                ('entry', 'entry-duplicate', 'upsert', '{"v":1}', NULL, '2026-08-20T00:00:00Z'),
                ('entry', 'entry-duplicate', 'upsert', '{"v":2}', '', '2026-08-21T00:00:00Z');
            "#,
        )
        .unwrap();
    }

    let queue = SyncQueue::open(temp.path()).unwrap();
    queue.init().unwrap();

    let rows = queue.pending(10, 5).unwrap();
    assert_eq!(rows.len(), 1, "legacy duplicate identities must collapse");
    assert_eq!(rows[0].payload.as_deref(), Some(r#"{"v":2}"#));
}

/// GH #652: an operator can retry only the parked rows whose diagnostic names
/// the repaired server reason, leaving unrelated terminal rows untouched.
#[test]
fn retry_failed_by_reason_requeues_only_matching_terminal_rows() {
    let (_temp, queue) = create_test_queue();
    const MAX_RETRIES: i32 = 5;

    for (id, reason) in [
        ("project-mismatch", "project_mismatch"),
        ("scope-mismatch", "scope_mismatch"),
    ] {
        queue
            .enqueue(EntityType::Task, id, SyncOperation::Upsert, Some("{}"))
            .unwrap();
        let row_id = queue
            .pending(10, MAX_RETRIES)
            .unwrap()
            .iter()
            .find(|row| row.entity_id == id)
            .unwrap()
            .id;
        for _ in 0..MAX_RETRIES {
            queue
                .mark_failed(row_id, &format!("server reason={reason}"))
                .unwrap();
        }
    }

    assert_eq!(
        queue
            .retry_failed_for_reason("project_mismatch", MAX_RETRIES)
            .unwrap(),
        1
    );
    let pending = queue.pending(10, MAX_RETRIES).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].entity_id, "project-mismatch");
    assert_eq!(
        queue
            .failed_for_entity_type(None, MAX_RETRIES, 10)
            .unwrap()
            .len(),
        1
    );
}

/// AC4: A row with team_id=NULL (inserted by an older code path that did not
/// normalise the personal-queue sentinel) must coalesce with a new personal-
/// queue enqueue (team_id='') instead of creating a duplicate.
///
/// Root cause (defect C / cas-8dd8): SQLite treats NULL != '' under UNIQUE,
/// so a row with team_id=NULL and a subsequent enqueue with team_id='' each
/// satisfy UNIQUE(entity_type, entity_id, team_id) independently and create
/// two rows for the same entity.  The fix adds an idempotent UPDATE at the end
/// of migrate_team_id() that normalises NULL→'' so the unique index can
/// deduplicate correctly on the next enqueue.
#[test]
fn test_null_team_id_normalized_to_empty_on_migration() {
    use rusqlite::Connection;

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("cas.db");

    // Step 1: Initialise the queue normally so the full schema (including
    // team_id column and indexes) is in place.
    {
        let queue = SyncQueue::open(temp.path()).unwrap();
        queue.init().unwrap();
    }

    // Step 2: Simulate a pre-normalisation state by directly inserting a row
    // with team_id=NULL.  This is the shape produced by an older code path
    // that used NULL as the personal-queue sentinel before the fix.
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            r#"INSERT INTO sync_queue
                (entity_type, entity_id, operation, payload, team_id, created_at, retry_count)
               VALUES
                ('task', 'task-dup', 'upsert', '{"id":"task-dup","v":1}', NULL, '2026-01-01T00:00:00Z', 0)"#,
            [],
        )
        .unwrap();
    }

    // Step 3: Re-open and call init() — migrate_team_id() ends with an
    // idempotent `UPDATE … SET team_id = '' WHERE team_id IS NULL` that turns
    // the legacy NULL row into a '' row, making the UNIQUE index cover it.
    let queue = SyncQueue::open(temp.path()).unwrap();
    queue.init().unwrap();

    // Step 4: Enqueue the same entity via the normal path (team_id='').
    // Before the fix: NULL != '' under UNIQUE → second row inserted (duplicate).
    // After the fix: both rows share team_id='' → ON CONFLICT coalesces to 1.
    queue
        .enqueue(
            EntityType::Task,
            "task-dup",
            SyncOperation::Upsert,
            Some(r#"{"id":"task-dup","v":2}"#),
        )
        .unwrap();

    let pending = queue.pending(10, 5).unwrap();
    assert_eq!(
        pending.len(),
        1,
        "NULL team_id must be normalised to '' so the UNIQUE constraint deduplicates — no duplicate (defect C / cas-8dd8)"
    );

    // Confirm the coalesced row holds the latest payload.
    assert!(
        pending[0].payload.as_ref().unwrap().contains("\"v\":2"),
        "coalesced row must hold the updated payload from the most-recent enqueue"
    );
}

#[test]
fn project_id_migration_preserves_legacy_rows_and_allows_move_pair() {
    use rusqlite::Connection;

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("cas.db");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE sync_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                operation TEXT NOT NULL,
                payload TEXT,
                team_id TEXT,
                created_at TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT,
                UNIQUE(entity_type, entity_id, team_id)
            );
            INSERT INTO sync_queue
                (entity_type, entity_id, operation, payload, team_id, created_at)
            VALUES ('task', 'legacy-task', 'upsert', '{"id":"legacy-task"}', 'team-123', '2026-01-01T00:00:00Z');
            "#,
        )
        .unwrap();
    }

    let queue = SyncQueue::open(temp.path()).unwrap();
    queue.init().unwrap();
    let legacy = queue.pending_for_team("team-123", 10, 5).unwrap();
    assert_eq!(legacy.len(), 1);
    assert_eq!(legacy[0].project_id, None);

    queue
        .enqueue_team_move(
            EntityType::Task,
            "move-after-migration",
            "project-a",
            "project-b",
            r#"{"id":"move-after-migration","origin_project":"project-b"}"#,
            "team-123",
        )
        .unwrap();
    let moved = queue.pending_for_team("team-123", 10, 5).unwrap();
    assert_eq!(moved.len(), 3);
    assert_eq!(moved[1].operation, SyncOperation::Delete);
    assert_eq!(moved[1].project_id.as_deref(), Some("project-a"));
    assert_eq!(moved[2].operation, SyncOperation::Upsert);
    assert_eq!(moved[2].project_id.as_deref(), Some("project-b"));
}

#[test]
fn enqueue_for_team_project_targets_a_foreign_owner() {
    let (_temp, queue) = create_test_queue();

    queue
        .enqueue_for_team_project(
            EntityType::Task,
            "foreign-task",
            SyncOperation::Upsert,
            Some(r#"{"id":"foreign-task"}"#),
            "team-123",
            Some("destination-project"),
        )
        .unwrap();

    let pending = queue.pending_for_team("team-123", 10, 5).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].project_id.as_deref(), Some("destination-project"));
}

#[test]
fn test_team_coalesce_updates() {
    let (_temp, queue) = create_test_queue();

    queue
        .enqueue_for_team(
            EntityType::Entry,
            "entry-1",
            SyncOperation::Upsert,
            Some(r#"{"content":"v1"}"#),
            "team-123",
        )
        .unwrap();

    queue
        .enqueue_for_team(
            EntityType::Entry,
            "entry-1",
            SyncOperation::Upsert,
            Some(r#"{"content":"v2"}"#),
            "team-123",
        )
        .unwrap();

    let pending = queue.pending_for_team("team-123", 10, 5).unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].payload.as_ref().unwrap().contains("v2"));
}

#[test]
fn dependency_tombstone_ledger_keeps_the_newest_delete_and_prunes_by_retention() {
    use chrono::{Duration, Utc};

    let temp = tempfile::TempDir::new().unwrap();
    let queue = SyncQueue::open(temp.path()).unwrap();
    queue.init().unwrap();

    let entity_id = "cas-a:cas-b:blocks";
    let older = Utc::now() - Duration::hours(5);
    let newer = Utc::now() - Duration::hours(1);
    queue
        .record_dependency_tombstone(entity_id, "cas-a", "cas-b", "blocks", newer)
        .unwrap();
    // A replayed older delete must not roll the ledger backwards.
    queue
        .record_dependency_tombstone(entity_id, "cas-a", "cas-b", "blocks", older)
        .unwrap();
    assert_eq!(
        queue
            .dependency_tombstone(entity_id)
            .unwrap()
            .unwrap()
            .timestamp(),
        newer.timestamp()
    );
    assert_eq!(queue.dependency_tombstones().unwrap().len(), 1);

    // A queued upsert for a tombstoned edge is dropped: the server refuses it
    // anyway, so retrying it forever is pure noise.
    queue
        .enqueue(
            EntityType::TaskDependency,
            entity_id,
            SyncOperation::Upsert,
            Some("{}"),
        )
        .unwrap();
    assert_eq!(queue.drop_queued_dependency_upsert(entity_id).unwrap(), 1);
    assert!(queue.pending(10, 5).unwrap().is_empty());

    // The cloud prunes tombstones after 90 days; the local ledger follows so it
    // cannot suppress an edge the cloud has already forgotten.
    queue
        .record_dependency_tombstone(
            "cas-c:cas-d:related",
            "cas-c",
            "cas-d",
            "related",
            Utc::now() - Duration::days(120),
        )
        .unwrap();
    assert_eq!(queue.prune_dependency_tombstones(Utc::now()).unwrap(), 1);
    assert!(queue.dependency_tombstone(entity_id).unwrap().is_some());
    assert!(
        queue
            .dependency_tombstone("cas-c:cas-d:related")
            .unwrap()
            .is_none()
    );
}

/// GH #668: a terminal row keeps the cloud's own verdict, so reporting can say
/// *why* the row is parked instead of quoting a free-text diagnostic.
#[test]
fn record_row_outcome_groups_terminal_rows_by_cloud_reason() {
    let (_temp, queue) = create_test_queue();
    const MAX_RETRIES: i32 = 5;

    for (id, reason) in [
        ("task-a", "project_mismatch"),
        ("task-b", "project_mismatch"),
        ("task-c", "revision_conflict"),
    ] {
        queue
            .enqueue(EntityType::Task, id, SyncOperation::Upsert, Some("{}"))
            .unwrap();
        let row_id = terminal_row_id(&queue, id, MAX_RETRIES);
        queue
            .park_failed(row_id, "cloud rejected", MAX_RETRIES)
            .unwrap();
        queue
            .record_row_outcome(row_id, "rejected", Some(reason))
            .unwrap();
    }

    // A transport failure never receives a per-row verdict and must not be
    // counted as a cloud rejection.
    queue
        .enqueue(
            EntityType::Task,
            "task-offline",
            SyncOperation::Upsert,
            Some("{}"),
        )
        .unwrap();
    let offline = terminal_row_id(&queue, "task-offline", MAX_RETRIES);
    queue
        .park_failed(offline, "Network error: connection refused", MAX_RETRIES)
        .unwrap();

    let counts = queue
        .rejected_reason_counts_for_entity_type(None, MAX_RETRIES)
        .unwrap();
    assert_eq!(counts.get("project_mismatch").copied(), Some(2));
    assert_eq!(counts.get("revision_conflict").copied(), Some(1));
    assert_eq!(
        counts.len(),
        2,
        "transport failures are not cloud rejections"
    );
}

fn terminal_row_id(queue: &SyncQueue, entity_id: &str, max_retries: i32) -> i64 {
    queue
        .pending(100, max_retries)
        .unwrap()
        .into_iter()
        .find(|row| row.entity_id == entity_id)
        .expect("row is still pending")
        .id
}

/// GH #668: rows an older client parked (including rows parked before the
/// client recorded its build at all) get exactly one fresh attempt after an
/// upgrade, and a permanent cloud rejection is never resurrected.
#[test]
fn stale_client_failures_requeue_once_per_upgrade() {
    use rusqlite::Connection;

    let temp = TempDir::new().unwrap();
    let queue = SyncQueue::open(temp.path()).unwrap();
    queue.init().unwrap();
    const MAX_RETRIES: i32 = 5;

    for id in ["task-429", "task-permanent", "task-retryable-reason"] {
        queue
            .enqueue(EntityType::Task, id, SyncOperation::Upsert, Some("{}"))
            .unwrap();
        let row_id = terminal_row_id(&queue, id, MAX_RETRIES);
        queue
            .park_failed(row_id, "parked by an older build", MAX_RETRIES)
            .unwrap();
        match id {
            "task-permanent" => queue
                .record_row_outcome(row_id, "rejected", Some("project_mismatch"))
                .unwrap(),
            "task-retryable-reason" => queue
                .record_row_outcome(row_id, "rejected", Some("revision_conflict"))
                .unwrap(),
            _ => {}
        }
    }

    // Simulate the on-disk state of a client that predates the stamp column.
    {
        let conn = Connection::open(temp.path().join("cas.db")).unwrap();
        conn.execute("UPDATE sync_queue SET failed_client_version = NULL", [])
            .unwrap();
    }

    // The build that records failures is this crate's own version, so the
    // test must ask the same question production does.
    let this_build = env!("CARGO_PKG_VERSION");
    assert_eq!(
        queue
            .requeue_stale_client_failures(this_build, MAX_RETRIES)
            .unwrap(),
        2,
        "the 429 row and the retryable rejection get one more attempt"
    );
    assert_eq!(
        queue
            .requeue_stale_client_failures(this_build, MAX_RETRIES)
            .unwrap(),
        0,
        "the same build must not requeue the same rows twice"
    );

    let pending = queue
        .pending(10, MAX_RETRIES)
        .unwrap()
        .into_iter()
        .map(|row| row.entity_id)
        .collect::<Vec<_>>();
    assert_eq!(pending, vec!["task-429", "task-retryable-reason"]);
    assert_eq!(
        queue
            .rejected_reason_counts_for_entity_type(None, MAX_RETRIES)
            .unwrap()
            .get("project_mismatch")
            .copied(),
        Some(1),
        "a permanent rejection stays parked with its reason"
    );

    // A later build finds the requeued rows terminal again and gives them one
    // more attempt; the same build does not.
    for id in ["task-429", "task-retryable-reason"] {
        let row_id = terminal_row_id(&queue, id, MAX_RETRIES);
        queue
            .park_failed(row_id, "still failing", MAX_RETRIES)
            .unwrap();
    }
    assert_eq!(
        queue
            .requeue_stale_client_failures(this_build, MAX_RETRIES)
            .unwrap(),
        0
    );
    assert_eq!(
        queue
            .requeue_stale_client_failures("99.0.0", MAX_RETRIES)
            .unwrap(),
        2
    );
}

/// A database created before the verdict columns existed gains them on init
/// without losing its queued rows.
#[test]
fn row_outcome_columns_are_added_to_legacy_databases() {
    use rusqlite::Connection;

    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("cas.db");
    {
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            r#"
            CREATE TABLE sync_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                operation TEXT NOT NULL,
                payload TEXT,
                team_id TEXT,
                project_id TEXT,
                created_at TEXT NOT NULL,
                retry_count INTEGER NOT NULL DEFAULT 0,
                last_error TEXT
            );
            INSERT INTO sync_queue
                (entity_type, entity_id, operation, payload, team_id, created_at, retry_count)
            VALUES ('task', 'legacy-task', 'upsert', '{}', '', '2026-01-01T00:00:00Z', 5);
            "#,
        )
        .unwrap();
    }

    let queue = SyncQueue::open(temp.path()).unwrap();
    queue.init().unwrap();

    assert_eq!(
        queue
            .rejected_reason_counts_for_entity_type(None, 5)
            .unwrap()
            .len(),
        0
    );
    assert_eq!(
        queue
            .requeue_stale_client_failures(env!("CARGO_PKG_VERSION"), 5)
            .unwrap(),
        1,
        "a row parked without a build stamp is an older-client failure"
    );
}
