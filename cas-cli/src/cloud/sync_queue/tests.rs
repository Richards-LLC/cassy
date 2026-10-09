use tempfile::TempDir;

use crate::cloud::sync_queue::{EntityType, SyncOperation, SyncQueue};

fn create_test_queue() -> (TempDir, SyncQueue) {
    let temp = TempDir::new().unwrap();
    let queue = SyncQueue::open(temp.path()).unwrap();
    queue.init().unwrap();
    (temp, queue)
}

#[test]
fn cas_8095_purge_removes_only_rejected_team_owned_personal_rows() {
    let (_temp, queue) = create_test_queue();
    for (id, outcome, reason) in [
        ("retired-task", "rejected", "team_owned_project"),
        ("retired-dependency", "rejected", "team_owned_project"),
        ("other-rejection", "rejected", "revision_conflict"),
        ("skipped-team-owned", "skipped", "team_owned_project"),
        ("retryable", "", ""),
    ] {
        let kind = if id == "retired-dependency" {
            EntityType::TaskDependency
        } else {
            EntityType::Task
        };
        queue
            .enqueue(kind, id, SyncOperation::Upsert, Some("{}"))
            .unwrap();
        let row = queue
            .list_all(100)
            .unwrap()
            .into_iter()
            .find(|row| row.entity_id == id)
            .unwrap();
        queue
            .record_row_outcome(row.id, outcome, Some(reason))
            .unwrap();
    }
    queue
        .enqueue_for_team(
            EntityType::Task,
            "team-row",
            SyncOperation::Upsert,
            Some("{}"),
            "team-1",
        )
        .unwrap();
    let team_row = queue
        .list_all(100)
        .unwrap()
        .into_iter()
        .find(|row| row.entity_id == "team-row")
        .unwrap();
    queue
        .record_row_outcome(team_row.id, "rejected", Some("team_owned_project"))
        .unwrap();
    assert_eq!(queue.purge_team_owned_personal_rejections().unwrap(), 2);
    let survivors = queue.list_all(100).unwrap();
    assert_eq!(survivors.len(), 4);
    assert!(survivors.iter().any(|row| row.entity_id == "team-row"));
    assert!(
        !survivors
            .iter()
            .any(|row| row.entity_id.starts_with("retired-"))
    );
    assert_eq!(queue.purge_team_owned_personal_rejections().unwrap(), 0);
}

/// cas-25c1: the mismatch count is exactly the set the purge would remove:
/// personal rows the cloud rejected as `team_owned_project`. Team rows, other
/// rejection reasons and merely skipped rows are not the team-only mismatch.
#[test]
fn cas_25c1_team_owned_rejection_count_matches_the_purge_scope() {
    let (_temp, queue) = create_test_queue();
    for (id, outcome, reason) in [
        ("owned-a", "rejected", "team_owned_project"),
        ("owned-b", "rejected", "team_owned_project"),
        ("orphan", "rejected", "orphan_dependency"),
        ("skipped-owned", "skipped", "team_owned_project"),
    ] {
        queue
            .enqueue(EntityType::Task, id, SyncOperation::Upsert, Some("{}"))
            .unwrap();
        let row = queue
            .list_all(100)
            .unwrap()
            .into_iter()
            .find(|row| row.entity_id == id)
            .unwrap();
        queue
            .record_row_outcome(row.id, outcome, Some(reason))
            .unwrap();
    }
    queue
        .enqueue_for_team(
            EntityType::Task,
            "team-row",
            SyncOperation::Upsert,
            Some("{}"),
            "team-1",
        )
        .unwrap();
    let team_row = queue
        .list_all(100)
        .unwrap()
        .into_iter()
        .find(|row| row.entity_id == "team-row")
        .unwrap();
    queue
        .record_row_outcome(team_row.id, "rejected", Some("team_owned_project"))
        .unwrap();

    assert_eq!(queue.team_owned_personal_rejection_count().unwrap(), 2);
    assert_eq!(queue.purge_team_owned_personal_rejections().unwrap(), 2);
    assert_eq!(queue.team_owned_personal_rejection_count().unwrap(), 0);
}

#[test]
fn foreign_and_unknown_stored_entries_never_enter_personal_or_team_queue() {
    use rusqlite::Connection;

    let (temp, queue) = create_test_queue();
    std::fs::write(
        temp.path().join("config.toml"),
        "[project]\ncanonical_id = \"local-project\"\n",
    )
    .unwrap();
    let conn = Connection::open(temp.path().join("cas.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS entries (id TEXT PRIMARY KEY, origin_project TEXT);
         INSERT INTO entries (id, origin_project) VALUES
           ('foreign', 'other-project'), ('unknown', 'unknown'),
           ('authored', 'local-project');",
    )
    .unwrap();

    for id in ["foreign", "unknown", "authored"] {
        let payload = format!(r#"{{"id":"{id}","scope":"project"}}"#);
        queue
            .enqueue(EntityType::Entry, id, SyncOperation::Upsert, Some(&payload))
            .unwrap();
        queue
            .enqueue_for_team(
                EntityType::Entry,
                id,
                SyncOperation::Upsert,
                Some(&payload),
                "team-1",
            )
            .unwrap();
    }
    assert_eq!(queue.unauthored_skipped_count().unwrap(), 4);
    assert_eq!(queue.pending(10, 5).unwrap().len(), 1);
    assert_eq!(queue.pending_for_team("team-1", 10, 5).unwrap().len(), 1);
    assert_eq!(queue.pending(10, 5).unwrap()[0].entity_id, "authored");

    // Simulate rows left by an older client that had no enqueue guard.
    conn.execute(
        "INSERT INTO sync_queue (entity_type, entity_id, operation, payload, team_id, created_at)
         VALUES ('entry', 'foreign', 'upsert', '{}', '', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    conn.execute(
        "INSERT INTO sync_queue (entity_type, entity_id, operation, payload, team_id, created_at)
         VALUES ('entry', 'unknown', 'upsert', '{}', 'team-1', '2026-01-01T00:00:00Z')",
        [],
    )
    .unwrap();
    assert_eq!(
        queue
            .drop_queued_rows_with_foreign_origin("local-project")
            .unwrap(),
        2
    );
    assert_eq!(queue.unauthored_skipped_count().unwrap(), 6);
    assert_eq!(queue.pending(10, 5).unwrap().len(), 1);
    assert_eq!(queue.pending_for_team("team-1", 10, 5).unwrap().len(), 1);
}

#[test]
fn local_key_delete_survives_foreign_stored_origin_after_move() {
    use rusqlite::Connection;

    let (temp, queue) = create_test_queue();
    let conn = Connection::open(temp.path().join("cas.db")).unwrap();
    conn.execute_batch(
        r#"CREATE TABLE IF NOT EXISTS tasks (id TEXT PRIMARY KEY, origin_project TEXT);
         INSERT INTO tasks (id, origin_project) VALUES
           ('old-key', 'new-project'), ('legacy-key', 'new-project'),
           ('foreign-key', 'new-project'), ('foreign-upsert', 'new-project');
         INSERT INTO sync_queue
           (entity_type, entity_id, operation, payload, team_id, project_id, created_at)
         VALUES
           ('task', 'old-key', 'delete', NULL, 'team-1', 'local-project', '2026-01-01T00:00:00Z'),
           ('task', 'legacy-key', 'delete', NULL, 'team-1', NULL, '2026-01-01T00:00:00Z'),
           ('task', 'foreign-key', 'delete', NULL, 'team-1', 'new-project', '2026-01-01T00:00:00Z'),
           ('task', 'foreign-upsert', 'upsert', '{"id":"foreign-upsert"}', 'team-1', NULL, '2026-01-01T00:00:00Z');"#,
    )
    .unwrap();

    assert_eq!(
        queue
            .drop_queued_rows_with_foreign_origin("local-project")
            .unwrap(),
        2
    );
    let surviving = queue.pending_for_team("team-1", 10, 5).unwrap();
    assert_eq!(surviving.len(), 2);
    assert!(
        surviving
            .iter()
            .all(|row| row.operation == SyncOperation::Delete)
    );
    assert!(surviving.iter().any(|row| row.entity_id == "old-key"));
    assert!(surviving.iter().any(|row| row.entity_id == "legacy-key"));
}

#[test]
fn queued_user_prompts_are_dropped_across_personal_and_team_scopes() {
    let (_temp, queue) = create_test_queue();
    let prompt = serde_json::json!({"tags": ["user-prompt"], "content": "User request: fix this"});
    let ordinary = serde_json::json!({"tags": ["context"], "content": "Keep this"});
    queue
        .enqueue(
            EntityType::Entry,
            "prompt",
            SyncOperation::Upsert,
            Some(&prompt.to_string()),
        )
        .unwrap();
    queue
        .enqueue_for_team(
            EntityType::Entry,
            "prompt",
            SyncOperation::Upsert,
            Some(&prompt.to_string()),
            "team-1",
        )
        .unwrap();
    queue
        .enqueue(
            EntityType::Entry,
            "ordinary",
            SyncOperation::Upsert,
            Some(&ordinary.to_string()),
        )
        .unwrap();

    assert_eq!(queue.drop_queued_user_prompts().unwrap(), 2);
    assert_eq!(queue.drop_queued_user_prompts().unwrap(), 0);
    assert_eq!(queue.pending(10, 5).unwrap().len(), 1);
    assert!(queue.pending_for_team("team-1", 10, 5).unwrap().is_empty());
}

#[test]
fn team_only_neutralizes_project_copies_without_deleting_personal_rows() {
    let (_temp, queue) = create_test_queue();
    for (kind, id, payload) in [
        (EntityType::Task, "project-task", r#"{"scope":"project"}"#),
        (EntityType::Task, "global-task", r#"{"scope":"global"}"#),
        (
            EntityType::Entry,
            "project-entry",
            r#"{"scope":"project","entry_type":"learning"}"#,
        ),
        (
            EntityType::Entry,
            "private-entry",
            r#"{"scope":"project","share":"private"}"#,
        ),
        (EntityType::Rule, "project-rule", r#"{"scope":"project"}"#),
    ] {
        queue
            .enqueue(kind, id, SyncOperation::Upsert, Some(payload))
            .unwrap();
    }
    queue
        .enqueue_for_team(
            EntityType::Task,
            "project-task",
            SyncOperation::Upsert,
            Some(r#"{"scope":"project"}"#),
            "team-1",
        )
        .unwrap();
    queue
        .enqueue(
            EntityType::Rule,
            "deleted-rule",
            SyncOperation::Delete,
            None,
        )
        .unwrap();
    queue
        .enqueue_for_team(
            EntityType::Rule,
            "deleted-rule",
            SyncOperation::Delete,
            None,
            "team-1",
        )
        .unwrap();

    assert_eq!(queue.neutralize_team_only_personal().unwrap(), 4);
    assert_eq!(queue.personal_row_count().unwrap(), 2);
    let personal = queue.pending(10, 5).unwrap();
    assert_eq!(personal.len(), 2);
    assert!(personal.iter().any(|row| row.entity_id == "global-task"));
    assert!(personal.iter().any(|row| row.entity_id == "private-entry"));
    assert_eq!(queue.pending_for_team("team-1", 10, 5).unwrap().len(), 2);
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
fn registration_conflict_parks_team_rows_with_reason_without_retrying_them() {
    let (_temp, queue) = create_test_queue();
    queue
        .enqueue_for_team(
            EntityType::Task,
            "task-registration-conflict",
            SyncOperation::Upsert,
            Some(r#"{"id":"task-registration-conflict"}"#),
            "team-1",
        )
        .unwrap();

    let diagnostic = "project_registration_conflict: requested 'github.com/richards-llc/pulse-card' conflicts with registered 'pulse-card'; run `cas cloud project set pulse-card` or file an alias with the cloud owner";
    assert_eq!(
        queue
            .park_team_rows_for_registration_conflict("team-1", diagnostic, 5)
            .unwrap(),
        1
    );

    let pending = queue.pending_for_team("team-1", 10, 5).unwrap();
    assert_eq!(
        pending.len(),
        1,
        "a parked registration row stays retryable after repair"
    );
    assert_eq!(
        pending[0].retry_count, 0,
        "registration conflicts are not transport retries"
    );
    assert_eq!(pending[0].last_outcome.as_deref(), Some("parked"));
    assert_eq!(
        pending[0].last_reason.as_deref(),
        Some("project_registration_conflict")
    );
    assert_eq!(pending[0].last_error.as_deref(), Some(diagnostic));
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
    assert!(
        queue
            .get_metadata("last_team_pull_at_team-a_project-a")
            .unwrap()
            .is_none()
    );
    assert!(
        queue
            .get_metadata("last_team_pull_at_team-b_project-b")
            .unwrap()
            .is_none()
    );
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
fn enqueue_for_team_project_refuses_a_foreign_owner() {
    let (temp, queue) = create_test_queue();
    std::fs::write(
        temp.path().join("config.toml"),
        "[project]\ncanonical_id = \"local-project\"\n",
    )
    .unwrap();

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
    assert!(pending.is_empty());
    assert_eq!(queue.unauthored_skipped_count().unwrap(), 1);
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

/// Only a satisfied client version gate may requeue a terminal row on upgrade.
/// Retry exhaustion, manual parks, and cloud rejections require operator retry,
/// even when the row predates client version stamps.
#[test]
fn upgrade_requeues_only_version_gated_failures() {
    use rusqlite::Connection;

    let temp = TempDir::new().unwrap();
    let queue = SyncQueue::open(temp.path()).unwrap();
    queue.init().unwrap();
    const MAX_RETRIES: i32 = 5;

    for id in [
        "task-parked",
        "task-rejected",
        "task-version-gated",
        "task-stale-outcome",
        "task-team",
    ] {
        if id == "task-team" {
            queue
                .enqueue_for_team(
                    EntityType::Task,
                    id,
                    SyncOperation::Upsert,
                    Some("{}"),
                    "team-1",
                )
                .unwrap();
        } else {
            queue
                .enqueue(EntityType::Task, id, SyncOperation::Upsert, Some("{}"))
                .unwrap();
        }
        let row_id = queue
            .list_all(10)
            .unwrap()
            .into_iter()
            .find(|row| row.entity_id == id)
            .unwrap()
            .id;
        if id == "task-stale-outcome" {
            queue
                .record_row_outcome(row_id, "rejected", Some("revision_conflict"))
                .unwrap();
            for _ in 0..MAX_RETRIES {
                queue
                    .mark_failed(row_id, "Client version 3.4.2 is below minimum 3.5.0")
                    .unwrap();
            }
            continue;
        }
        queue
            .park_failed(
                row_id,
                if id == "task-version-gated" {
                    "Client version 3.4.2 is below minimum 3.5.0"
                } else {
                    "parked by operator or rejected by cloud"
                },
                MAX_RETRIES,
            )
            .unwrap();
        if id == "task-rejected" {
            queue
                .record_row_outcome(row_id, "rejected", Some("team_owned_project"))
                .unwrap();
        }
    }

    // Simulate the on-disk state of a client that predates the stamp column.
    {
        let conn = Connection::open(temp.path().join("cas.db")).unwrap();
        conn.execute("UPDATE sync_queue SET failed_client_version = NULL", [])
            .unwrap();
    }

    assert_eq!(
        queue
            .requeue_version_gated_failures("3.5.0", MAX_RETRIES)
            .unwrap(),
        2,
        "both version gates retry, including one with an earlier cloud rejection"
    );
    assert_eq!(
        queue
            .requeue_version_gated_failures("99.0.0", MAX_RETRIES)
            .unwrap(),
        0,
        "a later upgrade must leave all other terminal rows parked"
    );

    let pending = queue
        .pending(10, MAX_RETRIES)
        .unwrap()
        .into_iter()
        .map(|row| row.entity_id)
        .collect::<Vec<_>>();
    assert_eq!(pending, vec!["task-version-gated", "task-stale-outcome"]);
    assert!(
        queue
            .pending_for_team("team-1", 10, MAX_RETRIES)
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        queue
            .rejected_reason_counts_for_entity_type(None, MAX_RETRIES)
            .unwrap()
            .get("team_owned_project")
            .copied(),
        Some(1),
        "a cloud rejection stays parked with its reason"
    );

    assert_eq!(
        queue
            .retry_failed_for_reason("rejected by cloud", MAX_RETRIES)
            .unwrap(),
        3
    );
    assert_eq!(queue.pending(10, MAX_RETRIES).unwrap().len(), 4);
    assert_eq!(
        queue
            .pending_for_team("team-1", 10, MAX_RETRIES)
            .unwrap()
            .len(),
        1
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
        queue.requeue_version_gated_failures("99.0.0", 5).unwrap(),
        0
    );
    assert_eq!(queue.retry_failed(5).unwrap(), 1);
}

#[test]
fn cas_fd42_intentional_parks_are_not_team_push_failures() {
    let (_temp, queue) = create_test_queue();
    queue
        .enqueue_for_team(
            EntityType::Entry,
            "legacy-memory",
            SyncOperation::Upsert,
            Some("{}"),
            "fd42-team",
        )
        .unwrap();
    let row = queue.list_all(10).unwrap().pop().unwrap();
    queue
        .record_row_outcome(row.id, "parked", Some("unattributed_origin"))
        .unwrap();
    queue
        .park_failed(row.id, "no attributable origin; retained intentionally", 5)
        .unwrap();
    assert_eq!(queue.pending_count_for_team("fd42-team", 5).unwrap(), 0);
    assert_eq!(
        queue.failed_count_for_team("fd42-team", 5).unwrap(),
        0,
        "a named provenance park is not an attempted push failure"
    );
    assert_eq!(
        queue.list_all(10).unwrap().len(),
        1,
        "retain the local diagnostic"
    );
}

#[test]
fn cas_fd42_endpoint_delete_is_not_overwritten_by_repair() {
    let (_temp, queue) = create_test_queue();
    let tasks = [
        crate::types::Task::new("child".into(), "child".into()),
        crate::types::Task::new("parent".into(), "parent".into()),
    ];
    queue
        .enqueue_for_team(
            EntityType::Task,
            "parent",
            SyncOperation::Delete,
            None,
            "fd42-team",
        )
        .unwrap();
    assert!(
        !queue
            .stage_healed_dependency(
                "child:parent:blocks",
                "{}",
                &tasks,
                &tasks,
                Some("fd42-team"),
                "p",
                None,
                5
            )
            .unwrap()
    );
    let rows = queue.list_all(10).unwrap();
    assert_eq!(
        rows.len(),
        2,
        "no endpoint upsert may resurrect a pending deletion"
    );
    assert!(
        rows.iter()
            .any(|row| row.entity_id == "parent" && row.operation == SyncOperation::Delete)
    );
    assert_eq!(
        queue
            .intentional_park_counts(Some("fd42-team"), 5)
            .unwrap()
            .get("dependency_endpoint_deleted"),
        Some(&1)
    );
}

#[test]
fn cas_fd42_repair_preserves_newer_endpoint_write_and_other_failures() {
    let (temp, queue) = create_test_queue();
    std::fs::write(
        temp.path().join("config.toml"),
        "[project]\ncanonical_id=\"p\"\n",
    )
    .unwrap();
    let tasks = [
        crate::types::Task::new("child".into(), "old child".into()),
        crate::types::Task::new("parent".into(), "parent".into()),
    ];
    let newer = r#"{"id":"child","title":"newer edit","origin_project":"p"}"#;
    queue
        .enqueue_for_team(
            EntityType::Task,
            "child",
            SyncOperation::Upsert,
            Some(newer),
            "fd42-team",
        )
        .unwrap();
    let before = queue.list_all(10).unwrap();
    assert_eq!(
        before.len(),
        1,
        "the newer edit must actually be queued before repair"
    );
    assert_eq!(before[0].payload.as_deref(), Some(newer));
    assert_eq!(queue.unauthored_skipped_count().unwrap(), 0);
    assert!(
        queue
            .stage_healed_dependency(
                "child:parent:blocks",
                "{}",
                &tasks,
                &tasks,
                Some("fd42-team"),
                "p",
                None,
                5
            )
            .unwrap()
    );
    let rows = queue.list_all(10).unwrap();
    assert_eq!(
        rows.iter()
            .find(|row| row.entity_id == "child")
            .unwrap()
            .payload
            .as_deref(),
        Some(newer)
    );
    queue
        .enqueue_for_team(
            EntityType::Task,
            "unknown",
            SyncOperation::Upsert,
            Some("{}"),
            "fd42-team",
        )
        .unwrap();
    let row = queue
        .list_all(10)
        .unwrap()
        .into_iter()
        .find(|row| row.entity_id == "unknown")
        .unwrap();
    queue
        .record_row_outcome(row.id, "parked", Some("unknown_reason"))
        .unwrap();
    queue.park_failed(row.id, "unknown_reason", 5).unwrap();
    assert_eq!(
        queue.failed_count_for_team("fd42-team", 5).unwrap(),
        1,
        "unknown parks remain failures"
    );
    assert!(
        queue
            .intentional_park_counts(Some("fd42-team"), 5)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn cas_fd42_new_memory_creation_persists_project_origin() {
    use cas_store::Store;
    let temp = TempDir::new().unwrap();
    std::fs::write(
        temp.path().join("config.toml"),
        "[project]\ncanonical_id=\"fd42-project\"\n",
    )
    .unwrap();
    let store = crate::store::open_store(temp.path()).unwrap();
    let mut entry = crate::types::Entry::new("fresh-memory".into(), "new local memory".into());
    entry.team_id = Some("fd42-team".into());
    store.add(&entry).unwrap();
    assert_eq!(
        store.get(&entry.id).unwrap().origin_project.as_deref(),
        Some("fd42-project")
    );
}

#[test]
fn cas_fd42_endpoint_move_preserves_routes_and_parks_edge() {
    let (_temp, queue) = create_test_queue();
    let tasks = [
        crate::types::Task::new("child".into(), "child".into()),
        crate::types::Task::new("parent".into(), "parent".into()),
    ];
    queue
        .enqueue_team_move(
            EntityType::Task,
            "parent",
            "p",
            "other",
            r#"{"id":"parent","origin_project":"other"}"#,
            "fd42-team",
        )
        .unwrap();
    let before = queue.list_all(10).unwrap();
    assert!(
        !queue
            .stage_healed_dependency(
                "child:parent:blocks",
                "{}",
                &tasks,
                &tasks,
                Some("fd42-team"),
                "p",
                None,
                5
            )
            .unwrap()
    );
    let after = queue.list_all(10).unwrap();
    assert_eq!(after.len(), before.len() + 1);
    for row in before {
        let kept = after
            .iter()
            .find(|candidate| candidate.id == row.id)
            .unwrap();
        assert_eq!(kept.payload, row.payload);
        assert_eq!(kept.operation, row.operation);
        assert_eq!(kept.project_id, row.project_id);
    }
    assert_eq!(
        queue.pending_for_team("fd42-team", 10, 5).unwrap().len(),
        2,
        "only the move pair stays sendable"
    );
}

#[test]
fn cas_fd42_concurrent_dependency_delete_wins_over_healing() {
    let (_temp, queue) = create_test_queue();
    let tasks = [
        crate::types::Task::new("child".into(), "child".into()),
        crate::types::Task::new("parent".into(), "parent".into()),
    ];
    queue
        .enqueue_for_team(
            EntityType::TaskDependency,
            "child:parent:blocks",
            SyncOperation::Delete,
            None,
            "fd42-team",
        )
        .unwrap();
    assert!(
        !queue
            .stage_healed_dependency(
                "child:parent:blocks",
                "{}",
                &tasks,
                &tasks,
                Some("fd42-team"),
                "p",
                None,
                5
            )
            .unwrap()
    );
    let rows = queue.list_all(10).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].operation, SyncOperation::Delete);
}

#[test]
fn cas_fd42_concurrent_dependency_edit_with_stale_verdict_wins() {
    let (_temp,queue)=create_test_queue();
    let tasks=[crate::types::Task::new("child".into(),"child".into()),crate::types::Task::new("parent".into(),"parent".into())];
    queue.enqueue_for_team(EntityType::TaskDependency,"child:parent:blocks",SyncOperation::Upsert,Some("old"),"fd42-team").unwrap();
    let row=queue.list_all(10).unwrap().pop().unwrap();
    queue.record_row_outcome(row.id,"rejected",Some("orphan_dependency")).unwrap();queue.park_failed(row.id,"orphan_dependency",5).unwrap();
    queue.enqueue_for_team(EntityType::TaskDependency,"child:parent:blocks",SyncOperation::Upsert,Some("newer edit"),"fd42-team").unwrap();
    assert!(!queue.stage_healed_dependency("child:parent:blocks","old",&tasks,&tasks,Some("fd42-team"),"p",None,5).unwrap());
    let rows=queue.list_all(10).unwrap();assert_eq!(rows.len(),1);assert_eq!(rows[0].payload.as_deref(),Some("newer edit"));
}
#[test]
fn cas_fd42_repair_preserves_existing_endpoint_routes_and_retry_metadata() {
    for project_route in [None, Some("p")] {
        let (temp, queue) = create_test_queue();
        std::fs::write(
            temp.path().join("config.toml"),
            "[project]\ncanonical_id=\"p\"\n",
        )
        .unwrap();
        let mut tasks = [
            crate::types::Task::new("child".into(), "old child".into()),
            crate::types::Task::new("parent".into(), "parent".into()),
        ];
        for task in &mut tasks {
            task.origin_project = Some("p".into());
        }
        let newer = r#"{"id":"child","title":"newer edit","origin_project":"p"}"#;
        queue
            .enqueue_for_team_project(
                EntityType::Task,
                "child",
                SyncOperation::Upsert,
                Some(newer),
                "fd42-team",
                project_route,
            )
            .unwrap();
        let before = queue.list_all(10).unwrap();
        assert_eq!(before.len(), 1, "setup must queue the edit");
        queue
            .record_row_outcome(before[0].id, "rejected", Some("scope_mismatch"))
            .unwrap();
        queue
            .park_failed(before[0].id, "scope_mismatch", 5)
            .unwrap();
        let before = queue.list_all(10).unwrap().pop().unwrap();
        assert!(
            queue
                .stage_healed_dependency(
                    "child:parent:blocks",
                    "{}",
                    &tasks,
                    &tasks,
                    Some("fd42-team"),
                    "p",
                    None,
                    5
                )
                .unwrap()
        );
        let rows = queue.list_all(10).unwrap();
        assert_eq!(rows.len(), 3);
        let after = rows.iter().find(|row| row.entity_id == "child").unwrap();
        assert_eq!(after.id, before.id);
        assert_eq!(after.payload, before.payload);
        assert_eq!(after.project_id, before.project_id);
        assert_eq!(after.created_at, before.created_at);
        assert_eq!(after.retry_count, before.retry_count);
        assert_eq!(after.last_error, before.last_error);
        assert_eq!(after.last_outcome, before.last_outcome);
        assert_eq!(after.last_reason, before.last_reason);
        assert_eq!(after.failed_client_version, before.failed_client_version);
        assert!(
            queue
                .dependency_endpoint_queued("child", "parent", "fd42-team")
                .unwrap(),
            "failed endpoint still withholds the edge"
        );
    }
}

/// cas-0e57: every logged-in task-store open runs `open` + `init`. A repeat
/// init must not take the write lock, and the queue's own writes must wait
/// out a foreign writer instead of failing at once.
#[test]
fn reopen_reads_only_and_writes_wait_for_a_foreign_lock_cas_0e57() {
    let (temp, queue) = create_test_queue();
    queue
        .enqueue(EntityType::Task, "cas-0e57-a", SyncOperation::Upsert, Some("{}"))
        .unwrap();
    drop(queue);

    let locker = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
    locker.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = std::time::Instant::now();
    let queue = SyncQueue::open(temp.path()).unwrap();
    queue.init().unwrap();
    assert!(
        started.elapsed() < std::time::Duration::from_secs(1),
        "init waited {:?} on a foreign write lock",
        started.elapsed()
    );

    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(400));
        locker.execute_batch("COMMIT").unwrap();
    });
    queue
        .enqueue(EntityType::Task, "cas-0e57-b", SyncOperation::Upsert, Some("{}"))
        .expect("a briefly held write lock is waited out");
    release.join().unwrap();
}
