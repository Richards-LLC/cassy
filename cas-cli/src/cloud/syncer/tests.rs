use std::sync::Arc;
use std::time::Duration;

use crate::cloud::SyncQueue;
use crate::cloud::syncer::*;
use crate::store::TaskStore;
use crate::types::{Task, TaskStatus};

#[test]
fn test_sync_result_totals() {
    let result = SyncResult {
        pushed_entries: 5,
        pushed_tasks: 3,
        pushed_rules: 2,
        pushed_skills: 1,
        pushed_sessions: 4,
        pulled_entries: 10,
        pulled_tasks: 5,
        pulled_rules: 0,
        pulled_skills: 2,
        ..Default::default()
    };

    assert_eq!(result.total_pushed(), 15); // 5+3+2+1+4
    assert_eq!(result.total_pulled(), 17);
    assert!(!result.has_errors());
}

#[test]
fn test_sync_result_with_sessions() {
    let result = SyncResult {
        pushed_sessions: 10,
        ..Default::default()
    };

    assert_eq!(result.total_pushed(), 10);
    assert_eq!(result.pushed_sessions, 10);
}

#[test]
fn test_sync_result_has_errors() {
    let mut result = SyncResult::default();
    assert!(!result.has_errors());

    result.errors.push("Test error".to_string());
    assert!(result.has_errors());
}

#[test]
fn heal_summary_is_quiet_for_noop_and_exact_when_edges_change() {
    assert_eq!(SyncResult::default().dependency_heal_summary(), None);

    let result = SyncResult {
        healed_task_dependencies_to_cloud: 2,
        healed_task_dependencies_from_cloud: 3,
        ..Default::default()
    };
    assert_eq!(
        result.dependency_heal_summary().as_deref(),
        Some("healed 2 edge(s) to cloud, 3 from cloud")
    );
}

#[test]
fn conflict_recording_tracks_directional_counts_and_details() {
    use chrono::Utc;

    let now = Utc::now();
    let mut result = SyncResult::default();
    result.record_conflict(SyncConflict {
        entity_type: "entry".to_string(),
        entity_id: "local-wins".to_string(),
        local_updated: now,
        remote_updated: now,
        local_revision: None,
        remote_revision: None,
        resolution: ConflictResolution::KeepRecent,
        action: ConflictAction::UseLocal,
    });
    result.record_conflict(SyncConflict {
        entity_type: "task".to_string(),
        entity_id: "remote-wins".to_string(),
        local_updated: now,
        remote_updated: now,
        local_revision: None,
        remote_revision: None,
        resolution: ConflictResolution::RemoteWins,
        action: ConflictAction::UseRemote,
    });

    assert_eq!(result.conflicts_resolved, 2);
    assert_eq!(result.conflicts_resolved_local, 1);
    assert_eq!(result.conflicts_resolved_remote, 1);
    assert_eq!(result.conflicts.len(), 2);

    let json = serde_json::to_value(&result).unwrap();
    assert_eq!(json["conflicts_resolved_local"], 1);
    assert_eq!(json["conflicts_resolved_remote"], 1);
}

#[test]
fn concise_errors_groups_parked_rejections_without_server_json() {
    let result = SyncResult {
        errors: vec![
            "permanent cloud rejection: reason=project_mismatch; entity=tasks; id=cas-1; existing_project=other".to_string(),
            "permanent cloud rejection: reason=project_mismatch; entity=tasks; id=cas-2; existing_project=other".to_string(),
            "cloud skipped 1 row; server response: {\"tasks\":{\"skipped\":1}}".to_string(),
        ],
        ..Default::default()
    };

    let summary = result.concise_errors().join("\n");
    assert!(summary.contains("project_mismatch: 2 item(s) parked"));
    assert!(summary.contains("cas-1, cas-2"));
    assert!(summary.contains("cas cloud queue --verbose"));
    assert!(!summary.contains("{\"tasks\""));
}

#[test]
fn test_config_defaults() {
    let config = CloudSyncerConfig::default();
    assert_eq!(config.timeout, Duration::from_secs(30));
    assert_eq!(config.max_retries, 5);
    assert_eq!(config.batch_size, 50);
}

#[test]
fn test_config_backoff_duration() {
    let config = CloudSyncerConfig::default();

    // First attempt: ~1000ms (plus jitter)
    let d0 = config.backoff_duration(0);
    assert!(d0.as_millis() >= 1000);
    assert!(d0.as_millis() < 1200); // Allow for jitter

    // Second attempt: ~2000ms
    let d1 = config.backoff_duration(1);
    assert!(d1.as_millis() >= 2000);

    // Third attempt: ~4000ms
    let d2 = config.backoff_duration(2);
    assert!(d2.as_millis() >= 4000);
}

#[test]
fn test_config_backoff_caps_at_max() {
    let config = CloudSyncerConfig::default();

    // Very high attempt should be capped at 2^6 = 64x
    let d_high = config.backoff_duration(100);
    // 1000 * 64 = 64000ms max (plus jitter)
    assert!(d_high.as_millis() < 70000);
}

#[test]
fn test_conflict_resolution_default() {
    let strategy = ConflictResolution::default();
    assert_eq!(strategy, ConflictResolution::RemoteWins);
}

#[test]
fn test_config_default_team_conflict_resolution() {
    let config = CloudSyncerConfig::default();
    assert_eq!(
        config.team_conflict_resolution,
        ConflictResolution::RemoteWins
    );
}

#[test]
fn new_binds_project_identity_to_queue_root() {
    let temp = tempfile::tempdir().unwrap();
    crate::cloud::set_canonical_id_in_config_toml(temp.path(), "root-project").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());

    let syncer = CloudSyncer::new(
        queue,
        crate::cloud::CloudConfig::default(),
        CloudSyncerConfig::default(),
    );

    assert_eq!(
        syncer.personal_push_project_id().unwrap(),
        "root-project",
        "a syncer must resolve identity from its queue root, not process cwd"
    );
}

#[test]
fn test_conflict_action_variants() {
    // Test all ConflictAction variants exist
    let _use_remote = ConflictAction::UseRemote;
    let _use_local = ConflictAction::UseLocal;
    let _skip = ConflictAction::Skip;
}

#[test]
fn test_sync_conflict_creation() {
    use chrono::Utc;
    let conflict = SyncConflict {
        entity_type: "entry".to_string(),
        entity_id: "test-123".to_string(),
        local_updated: Utc::now(),
        remote_updated: Utc::now(),
        local_revision: None,
        remote_revision: None,
        resolution: ConflictResolution::RemoteWins,
        action: ConflictAction::UseRemote,
    };

    assert_eq!(conflict.entity_type, "entry");
    assert_eq!(conflict.entity_id, "test-123");
    assert_eq!(conflict.resolution, ConflictResolution::RemoteWins);
    assert_eq!(conflict.action, ConflictAction::UseRemote);

    // Should not panic
    conflict.log();
}

// ---------------------------------------------------------------------------
// PushResponse: defensive client read of server-side cross-project skips.
// cas-f645 — paired with cas-d656 / cas-0bdc on the server.
// ---------------------------------------------------------------------------

#[test]
fn push_response_default_has_no_skipped_field() {
    // Sanity: the legacy "trust the 200" path relies on the default having
    // `skipped == None` so skipped_count_for(_) returns 0.
    let resp = PushResponse::default();
    assert!(resp.skipped.is_none());
    assert_eq!(resp.skipped_count_for("entries"), Ok(0));
    assert_eq!(resp.skipped_count_for("tasks"), Ok(0));
}

#[test]
fn push_response_parses_skipped_field() {
    // The forward-looking wire shape: server returns a per-entity-type map.
    let body = r#"{"skipped":{"entries":3,"tasks":0}}"#;
    let resp: PushResponse = serde_json::from_str(body).expect("must parse");
    assert_eq!(resp.skipped_count_for("entries"), Ok(3));
    // Explicit 0 in the map is still reported as 0.
    assert_eq!(resp.skipped_count_for("tasks"), Ok(0));
    // Entity types not in the map are also 0 — distinguishes "no skip" from
    // "we never sent any of these" downstream.
    assert_eq!(resp.skipped_count_for("rules"), Ok(0));
}

#[test]
fn push_response_parses_live_nested_skipped_field() {
    let body = r#"{"tasks":{"inserted":0,"updated":0,"skipped":1}}"#;
    let resp: PushResponse = serde_json::from_str(body).expect("must parse live response");
    assert_eq!(resp.skipped_count_for("tasks"), Ok(1));
    assert_eq!(resp.skipped_count_for("entries"), Ok(0));
}

#[test]
fn push_response_rejects_unrecognized_or_conflicting_skip_signals() {
    let malformed: PushResponse = serde_json::from_str(r#"{"tasks":{"skipped":"one"}}"#).unwrap();
    assert!(malformed.skipped_count_for("tasks").is_err());

    let conflicting: PushResponse = serde_json::from_str(
        r#"{"skipped":{"tasks":1},"tasks":{"inserted":0,"updated":0,"skipped":2}}"#,
    )
    .unwrap();
    assert!(conflicting.skipped_count_for("tasks").is_err());
}

#[test]
fn itemized_rejections_allow_a_subset_of_skipped_rows() {
    let entity = serde_json::json!({
        "inserted": 0,
        "updated": 0,
        "skipped": 20,
        "rejected": (0..6)
            .map(|index| serde_json::json!({
                "id": format!("cas-rejected-{index}"),
                "reason": "scope_mismatch",
                "existing_canonical_id": "cas-src",
            }))
            .collect::<Vec<_>>()
    });
    let queued_ids = (0..20).map(|index| format!("cas-rejected-{index}"));

    let itemized = itemized_rejections_for(&entity, "tasks", 20, queued_ids)
        .expect("a well-formed rejection subset must be accepted")
        .expect("the rejection list is present");

    assert_eq!(itemized.len(), 6);
}

#[test]
fn itemized_rejections_fail_closed_when_malformed() {
    let rejection = |id: &str| {
        serde_json::json!({
            "id": id,
            "reason": "scope_mismatch",
            "existing_canonical_id": "cas-src",
        })
    };

    let over_count = serde_json::json!({
        "rejected": [rejection("cas-a"), rejection("cas-b")]
    });
    assert!(
        itemized_rejections_for(
            &over_count,
            "tasks",
            1,
            ["cas-a".to_string(), "cas-b".to_string()].into_iter(),
        )
        .unwrap_err()
        .contains("exceeds skipped count")
    );

    let unknown = serde_json::json!({"rejected": [rejection("cas-unknown")]});
    assert!(
        itemized_rejections_for(&unknown, "tasks", 1, ["cas-known".to_string()].into_iter())
            .unwrap_err()
            .contains("was not in this sub-batch")
    );

    let duplicate = serde_json::json!({
        "rejected": [rejection("cas-a"), rejection("cas-a")]
    });
    assert!(
        itemized_rejections_for(&duplicate, "tasks", 2, ["cas-a".to_string()].into_iter())
            .unwrap_err()
            .contains("duplicate id")
    );
}

#[test]
fn push_response_round_trips_optional_invalid_sibling() {
    let with_invalid = r#"{
        "tasks": {
            "inserted": 1,
            "updated": 0,
            "skipped": 1,
            "invalid": [{
                "id": "cas-malformed-revision",
                "reason": "invalid_revision",
                "detail": "updated_at must be an RFC3339 timestamp"
            }]
        }
    }"#;
    let parsed: PushResponse = serde_json::from_str(with_invalid).expect("must parse invalid[]");
    let failures = parsed
        .itemized_failures_for(
            "tasks",
            parsed.skipped_count_for("tasks").unwrap(),
            [
                "cas-valid".to_string(),
                "cas-malformed-revision".to_string(),
            ]
            .into_iter(),
        )
        .expect("invalid[] must be a valid itemized failure")
        .expect("invalid[] is present");

    assert_eq!(failures.len(), 1);
    let failure = failures
        .get("cas-malformed-revision")
        .expect("invalid row is mapped to its local queue id");
    assert_eq!(failure.id(), "cas-malformed-revision");
    match failure {
        PushItemizedFailure::Invalid(invalid) => {
            assert_eq!(invalid.reason.as_str(), "invalid_revision");
            assert_eq!(
                invalid.detail,
                serde_json::json!("updated_at must be an RFC3339 timestamp")
            );
        }
        PushItemizedFailure::Rejection(_) => panic!("invalid[] must not become rejected[]"),
    }

    // Omitting the new sibling is byte-for-byte the existing behavior: the
    // parsed response has no itemized failures and callers retain their
    // legacy aggregate/2xx handling.
    let without_invalid = r#"{"tasks":{"inserted":2,"updated":0,"skipped":0}}"#;
    let parsed: PushResponse =
        serde_json::from_str(without_invalid).expect("response without invalid[] must still parse");
    assert_eq!(parsed.skipped_count_for("tasks"), Ok(0));
    assert!(
        parsed
            .itemized_failures_for("tasks", 0, ["cas-valid".to_string()].into_iter())
            .unwrap()
            .is_none()
    );
}

#[test]
fn itemized_invalids_fail_closed_for_unknown_or_duplicate_rows() {
    let invalid = |id: &str| {
        serde_json::json!({
            "id": id,
            "reason": "invalid_revision",
            "detail": "malformed revision"
        })
    };

    let unknown = serde_json::json!({"invalid": [invalid("cas-unknown")]});
    assert!(
        itemized_invalids_for(&unknown, "tasks", 1, ["cas-known".to_string()].into_iter())
            .unwrap_err()
            .contains("was not in this sub-batch")
    );

    let duplicate = serde_json::json!({
        "invalid": [invalid("cas-a"), invalid("cas-a")]
    });
    assert!(
        itemized_invalids_for(&duplicate, "tasks", 2, ["cas-a".to_string()].into_iter())
            .unwrap_err()
            .contains("duplicate id")
    );
}

#[test]
fn team_response_parses_optional_invalid_sibling() {
    let body = r#"{
        "synced": {
            "entries": {
                "inserted": 0,
                "updated": 0,
                "skipped": 1,
                "invalid": [{
                    "id": "cas-invalid-entry",
                    "reason": "invalid_revision",
                    "detail": "revision is not monotonic"
                }]
            }
        }
    }"#;
    let response: TeamPushResponse = serde_json::from_str(body).expect("must parse team invalid[]");
    let entity = response
        .synced
        .as_object()
        .and_then(|synced| synced.get("entries"))
        .expect("team response keeps the entries result object");
    let failures = itemized_failures_for(
        entity,
        "synced.entries",
        1,
        ["cas-invalid-entry".to_string()].into_iter(),
    )
    .expect("team invalid[] must be a valid itemized failure")
    .expect("team invalid[] is present");
    assert!(matches!(
        failures.get("cas-invalid-entry"),
        Some(PushItemizedFailure::Invalid(invalid))
            if invalid.reason.as_str() == "invalid_revision"
                && invalid.detail == serde_json::json!("revision is not monotonic")
    ));
}

#[tokio::test]
async fn personal_push_keeps_itemized_invalid_revision_visible_in_queue_health() {
    use std::sync::Arc;

    use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": {
                "inserted": 1,
                "updated": 0,
                "skipped": 1,
                "invalid": [{
                    "id": "cas-invalid-entry",
                    "reason": "invalid_revision",
                    "detail": "updated_at must be an RFC3339 timestamp"
                }]
            }
        })))
        .mount(&server)
        .await;

    let temp = tempdir().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
    queue.init().unwrap();
    queue
        .enqueue(
            EntityType::Entry,
            "cas-valid-entry",
            SyncOperation::Upsert,
            Some(r#"{"id":"cas-valid-entry"}"#),
        )
        .unwrap();
    queue
        .enqueue(
            EntityType::Entry,
            "cas-invalid-entry",
            SyncOperation::Upsert,
            Some(r#"{"id":"cas-invalid-entry"}"#),
        )
        .unwrap();

    let syncer = CloudSyncer::new_for_project(
        queue.clone(),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
        "test-project".to_string(),
        temp.path(),
    );
    let result = syncer.push_scoped(PushScope::EntriesOnly).unwrap();

    assert_eq!(
        result.pushed_entries, 1,
        "the valid neighbour is still reported as pushed"
    );
    assert_eq!(result.errors.len(), 1);
    assert!(result.errors[0].contains("cas-invalid-entry"));
    assert!(result.errors[0].contains("invalid_revision"));
    assert!(result.errors[0].contains("updated_at must be an RFC3339 timestamp"));

    let health = queue.health(5, chrono::Utc::now()).unwrap();
    assert!(health.last_error.unwrap().contains("cas-invalid-entry"));
    let remaining = queue.list_all(10).unwrap();
    assert_eq!(remaining.len(), 1, "the accepted neighbor is settled");
    assert_eq!(remaining[0].entity_id, "cas-invalid-entry");
    assert_eq!(remaining[0].retry_count, 1);
}

#[tokio::test]
async fn team_push_serializes_task_dependency_collection() {
    use crate::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig, EntityType, SyncOperation};
    use flate2::read::GzDecoder;
    use std::io::Read;
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let team_id = "team-cas-616e";
    Mock::given(method("POST"))
        .and(path(format!("/api/teams/{team_id}/sync/push")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "synced": {
                "task_dependencies": {
                    "inserted": 1,
                    "updated": 0,
                    "skipped": 0
                }
            }
        })))
        .expect(1)
        .mount(&server)
        .await;

    let temp = tempdir().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
    queue.init().unwrap();
    queue
        .enqueue_for_team(
            EntityType::TaskDependency,
            "cas-616e-from:cas-616e-to:blocks",
            SyncOperation::Upsert,
            Some(
                r#"{"from_id":"cas-616e-from","to_id":"cas-616e-to","dep_type":"blocks","created_at":"2026-09-01T12:00:00Z"}"#,
            ),
            team_id,
        )
        .unwrap();

    let syncer = CloudSyncer::new(
        queue.clone(),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
    );
    let result = tokio::task::spawn_blocking(move || syncer.push_team(team_id))
        .await
        .unwrap()
        .unwrap();

    assert_eq!(result.pushed_task_dependencies, 1);
    assert!(queue.pending_for_team(team_id, 10, 5).unwrap().is_empty());

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let mut decoder = GzDecoder::new(requests[0].body.as_slice());
    let mut decoded = Vec::new();
    decoder.read_to_end(&mut decoded).unwrap();
    let payload: serde_json::Value = serde_json::from_slice(&decoded).unwrap();
    assert_eq!(payload["task_dependencies"][0]["from_id"], "cas-616e-from");
    assert_eq!(payload["task_dependencies"][0]["dep_type"], "blocks");
}

#[test]
fn push_response_is_backward_compatible_with_legacy_payload() {
    // Older cloud builds may return shapes like {"synced": {...}} or just
    // an empty body. Either must deserialize into a PushResponse whose
    // skipped field is None, so the client falls back to legacy
    // mark-synced behavior rather than treating the absence as "all
    // skipped".
    let legacy_synced_shape = r#"{"synced":{"entries":5,"tasks":0,"rules":0,"skills":0,"sessions":0,
                     "verifications":0,"events":0,"prompts":0,"file_changes":0,
                     "commit_links":0,"agents":0,"worktrees":0}}"#;
    let resp: PushResponse = serde_json::from_str(legacy_synced_shape)
        .expect("legacy {synced:...} body must still deserialize");
    assert!(resp.skipped.is_none());
    assert_eq!(resp.skipped_count_for("entries"), Ok(0));

    // Truly empty object — same expectation.
    let resp: PushResponse =
        serde_json::from_str("{}").expect("empty JSON object must deserialize");
    assert!(resp.skipped.is_none());
    assert_eq!(resp.skipped_count_for("entries"), Ok(0));
}

#[test]
fn push_response_skipped_count_threshold_drives_warn_path() {
    // This is the contract `push_batch` reads to decide whether to call
    // `mark_synced`: any non-zero count for the targeted entity type
    // triggers the warn-and-skip path; zero (or absent) triggers the
    // legacy mark-synced path. The test locks in the threshold so future
    // refactors of `skipped_count_for` can't silently change the gate.
    let body = r#"{"skipped":{"entries":1}}"#;
    let resp: PushResponse = serde_json::from_str(body).unwrap();
    assert!(
        resp.skipped_count_for("entries").unwrap() > 0,
        "warn-path must fire"
    );
    assert_eq!(
        resp.skipped_count_for("tasks"),
        Ok(0),
        "non-targeted entity types must not fire the warn-path"
    );
}

#[test]
fn push_response_parses_complete_per_row_outcomes() {
    let body = r#"{
        "tasks": {
            "inserted": 1,
            "updated": 1,
            "skipped": 1,
            "rows": [
                {"id": "task-inserted", "outcome": "inserted"},
                {"id": "task-updated", "outcome": "updated"},
                {"id": "task-skipped", "outcome": "skipped_lww"},
                {"id": "task-rejected", "outcome": "rejected", "reason": "project_mismatch"}
            ]
        }
    }"#;
    let response: PushResponse = serde_json::from_str(body).expect("per-row response parses");
    let rows = response
        .row_results_for(
            "tasks",
            [
                "task-inserted".to_string(),
                "task-updated".to_string(),
                "task-skipped".to_string(),
                "task-rejected".to_string(),
            ]
            .into_iter(),
        )
        .expect("row response validates")
        .expect("rows are present");

    assert!(matches!(
        rows["task-inserted"].outcome,
        PushRowOutcome::Inserted
    ));
    assert!(matches!(
        rows["task-updated"].outcome,
        PushRowOutcome::Updated
    ));
    assert!(matches!(
        rows["task-skipped"].outcome,
        PushRowOutcome::SkippedLww
    ));
    assert_eq!(
        rows["task-rejected"].reason.as_deref(),
        Some("project_mismatch")
    );
}

#[test]
fn push_response_rejects_incomplete_or_duplicate_per_row_outcomes() {
    let incomplete: PushResponse = serde_json::from_str(
        r#"{"entries":{"rows":[{"id":"entry-one","outcome":"inserted"}]}}"#,
    )
    .unwrap();
    let error = incomplete
        .row_results_for(
            "entries",
            ["entry-one".to_string(), "entry-two".to_string()].into_iter(),
        )
        .unwrap_err();
    assert!(error.contains("missing row entry-two"));

    let duplicate: PushResponse = serde_json::from_str(
        r#"{"entries":{"rows":[
            {"id":"entry-one","outcome":"inserted"},
            {"id":"entry-one","outcome":"updated"}
        ]}}"#,
    )
    .unwrap();
    assert!(duplicate
        .row_results_for("entries", ["entry-one".to_string()].into_iter())
        .unwrap_err()
        .contains("duplicate id"));
}

#[tokio::test]
async fn push_response_aggregate_skip_acknowledges_the_whole_personal_batch() {
    use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": {"inserted": 1, "updated": 0, "skipped": 1}
        })))
        .mount(&server)
        .await;

    let temp = tempdir().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
    queue.init().unwrap();
    queue
        .enqueue(
            EntityType::Entry,
            "entry-accepted",
            SyncOperation::Upsert,
            Some(r#"{"id":"entry-accepted"}"#),
        )
        .unwrap();
    queue
        .enqueue(
            EntityType::Entry,
            "entry-lww-skipped",
            SyncOperation::Upsert,
            Some(r#"{"id":"entry-lww-skipped"}"#),
        )
        .unwrap();

    let syncer = CloudSyncer::new_for_project(
        queue.clone(),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
        "test-project".to_string(),
        temp.path(),
    );
    let result = syncer.push_scoped(PushScope::EntriesOnly).unwrap();

    assert!(result.errors.is_empty(), "LWW skips are acknowledgements");
    assert_eq!(result.pushed_entries, 2);
    assert!(queue.list_all(10).unwrap().is_empty());
}

#[tokio::test]
async fn push_response_per_row_rejection_is_parked_without_poisoning_neighbors() {
    use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": {
                "inserted": 1,
                "updated": 0,
                "skipped": 1,
                "rows": [
                    {"id": "entry-good", "outcome": "updated"},
                    {"id": "entry-rejected", "outcome": "rejected", "reason": "project_mismatch"}
                ]
            }
        })))
        .mount(&server)
        .await;

    let temp = tempdir().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
    queue.init().unwrap();
    for id in ["entry-good", "entry-rejected"] {
        queue
            .enqueue(
                EntityType::Entry,
                id,
                SyncOperation::Upsert,
                Some(&format!(r#"{{"id":"{id}"}}"#)),
            )
            .unwrap();
    }

    let syncer = CloudSyncer::new_for_project(
        queue.clone(),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
        "test-project".to_string(),
        temp.path(),
    );
    let result = syncer.push_scoped(PushScope::EntriesOnly).unwrap();

    // GH #668: a partially rejected batch reports both halves. The accepted
    // neighbour is counted as pushed and the refusal is still an error, so a
    // per-row rejection no longer erases the rows the cloud did write.
    assert_eq!(result.pushed_entries, 1, "the accepted neighbour is reported");
    assert_eq!(result.errors.len(), 1);
    let remaining = queue.list_all(10).unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].entity_id, "entry-rejected");
    assert_eq!(remaining[0].retry_count, 5);
    assert!(remaining[0]
        .last_error
        .as_deref()
        .is_some_and(|error| error.contains("project_mismatch")));
}

#[test]
fn version_gate_requeues_only_after_minimum_and_is_idempotent() {
    let (_temp, queue) = {
        let temp = tempfile::tempdir().unwrap();
        // Pin the scratch root: the ephemeral-project guard refuses an unpinned
        // root under the temp directory, and a TempDir is exactly that.
        std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
        let queue = SyncQueue::open(temp.path()).unwrap();
        queue.init().unwrap();
        queue
            .enqueue(
                crate::cloud::EntityType::Task,
                "task-old-client",
                crate::cloud::SyncOperation::Upsert,
                Some(r#"{"id":"task-old-client"}"#),
            )
            .unwrap();
        queue
            .enqueue(
                crate::cloud::EntityType::Task,
                "task-new-enough",
                crate::cloud::SyncOperation::Upsert,
                Some(r#"{"id":"task-new-enough"}"#),
            )
            .unwrap();
        let ids = queue
            .pending(10, 5)
            .unwrap()
            .into_iter()
            .map(|item| item.id)
            .collect::<Vec<_>>();
        for _ in 0..5 {
            queue
                .mark_failed(
                    ids[0],
                    "Team push failed with status 400: Client version 3.4.2 is below minimum 3.5.0",
                )
                .unwrap();
            queue
                .mark_failed(
                    ids[1],
                    "Team push failed with status 400: Client version 3.4.2 is below minimum 3.4.9",
                )
                .unwrap();
        }
        (temp, queue)
    };

    assert_eq!(
        queue
            .requeue_version_gated_failures("3.4.8", 5)
            .unwrap(),
        0
    );
    assert!(queue
        .list_all(10)
        .unwrap()
        .iter()
        .all(|item| item.retry_count == 5 && item.last_error.is_some()));

    assert_eq!(
        queue
            .requeue_version_gated_failures("3.5.0", 5)
            .unwrap(),
        2
    );
    let requeued = queue.list_all(10).unwrap();
    assert!(requeued.iter().all(|item| {
        item.retry_count == 0 && item.last_error.is_none()
    }));
    assert_eq!(
        queue
            .requeue_version_gated_failures("3.5.0", 5)
            .unwrap(),
        0
    );
}

#[tokio::test]
async fn version_gate_push_requeues_terminal_items_before_reading_pending_queue() {
    use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": {"inserted": 1, "updated": 0, "skipped": 0}
        })))
        .mount(&server)
        .await;

    let temp = tempdir().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
    queue.init().unwrap();
    queue
        .enqueue(
            EntityType::Entry,
            "entry-version-gated",
            SyncOperation::Upsert,
            Some(r#"{"id":"entry-version-gated"}"#),
        )
        .unwrap();
    let id = queue.pending(10, 5).unwrap()[0].id;
    for _ in 0..5 {
        queue
            .mark_failed(
                id,
                "Push failed with status 400: Client version 3.4.2 is below minimum 3.5.0",
            )
            .unwrap();
    }

    let syncer = CloudSyncer::new_for_project(
        queue.clone(),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
        "test-project".to_string(),
        temp.path(),
    );
    let result = syncer.push_scoped(PushScope::EntriesOnly).unwrap();

    assert!(result.errors.is_empty());
    assert_eq!(result.pushed_entries, 1);
    assert!(queue.list_all(10).unwrap().is_empty());
}

// ---------------------------------------------------------------------------
// Cross-project team-task ownership (cas-2125).
// ---------------------------------------------------------------------------

fn team_task_fixture(
    id: &str,
    status: TaskStatus,
    project_id: &str,
    origin_project: &str,
    updated_at: chrono::DateTime<chrono::Utc>,
) -> serde_json::Value {
    let mut task = Task::new(id.to_string(), format!("task {id}"));
    task.status = status;
    task.updated_at = updated_at;
    if task.is_terminal() {
        task.closed_at = Some(updated_at);
        task.close_reason = Some("owner closed".to_string());
    }
    let mut raw = serde_json::to_value(task).unwrap();
    raw["project_id"] = serde_json::json!(project_id);
    raw["origin_project"] = serde_json::json!(origin_project);
    raw
}

fn team_dependency_fixture(
    dependency: &crate::types::Dependency,
    project_id: &str,
    operation: Option<&str>,
) -> serde_json::Value {
    let mut raw = serde_json::to_value(dependency).unwrap();
    raw["id"] = serde_json::json!(format!(
        "{}:{}:{}",
        dependency.from_id, dependency.to_id, dependency.dep_type
    ));
    raw["project_id"] = serde_json::json!(project_id);
    if let Some(operation) = operation {
        raw["operation"] = serde_json::json!(operation);
    }
    raw
}

async fn pull_team_task_fixtures(
    project_id: &str,
    tasks: Vec<serde_json::Value>,
    local_task: Option<Task>,
) -> (
    tempfile::TempDir,
    SyncResult,
    Arc<dyn TaskStore>,
    Arc<SyncQueue>,
) {
    pull_team_task_and_dependency_fixtures(
        project_id,
        tasks,
        local_task.into_iter().collect(),
        Vec::new(),
        Vec::new(),
    )
    .await
}

async fn pull_team_task_and_dependency_fixtures(
    project_id: &str,
    tasks: Vec<serde_json::Value>,
    local_tasks: Vec<Task>,
    local_dependencies: Vec<crate::types::Dependency>,
    task_dependencies: Vec<serde_json::Value>,
) -> (
    tempfile::TempDir,
    SyncResult,
    Arc<dyn TaskStore>,
    Arc<SyncQueue>,
) {
    pull_team_task_and_dependency_fixtures_with_pull_count(
        project_id,
        tasks,
        local_tasks,
        local_dependencies,
        task_dependencies,
        1,
    )
    .await
}

async fn pull_team_task_and_dependency_fixtures_with_pull_count(
    project_id: &str,
    tasks: Vec<serde_json::Value>,
    local_tasks: Vec<Task>,
    local_dependencies: Vec<crate::types::Dependency>,
    task_dependencies: Vec<serde_json::Value>,
    pull_count: usize,
) -> (
    tempfile::TempDir,
    SyncResult,
    Arc<dyn TaskStore>,
    Arc<SyncQueue>,
) {
    use crate::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig};
    use crate::store::{
        open_rule_store_local, open_skill_store_local, open_store_local, open_task_store_local,
    };
    use tempfile::TempDir;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let team_id = "team-cas-2125";
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{team_id}/sync/pull")))
        .and(query_param("project_id", project_id))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [],
            "tasks": tasks,
            "rules": [],
            "skills": [],
            "task_dependencies": task_dependencies,
            "pulled_at": "2026-09-01T12:00:00Z",
            "team_id": team_id,
            "status": "ok",
        })))
        .expect(pull_count as u64)
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
    queue.init().unwrap();
    let store = open_store_local(temp.path()).unwrap();
    let task_store = open_task_store_local(temp.path()).unwrap();
    for local_task in local_tasks {
        task_store.add(&local_task).unwrap();
    }
    for dependency in local_dependencies {
        task_store.add_dependency(&dependency).unwrap();
    }
    let rule_store = open_rule_store_local(temp.path()).unwrap();
    let skill_store = open_skill_store_local(temp.path()).unwrap();
    let syncer = CloudSyncer::new(
        Arc::clone(&queue),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
    );
    let mut result = SyncResult::default();
    for _ in 0..pull_count {
        result = syncer
            .pull_team(
                team_id,
                project_id,
                store.as_ref(),
                task_store.as_ref(),
                rule_store.as_ref(),
                skill_store.as_ref(),
            )
            .unwrap();
    }
    (temp, result, task_store, queue)
}

#[tokio::test]
async fn team_pull_applies_and_deletes_task_dependencies_without_requeue() {
    use crate::types::{Dependency, DependencyType};

    let project_id = "dependency-project";
    let from = Task::new("cas-616e-from".to_string(), "from".to_string());
    let to = Task::new("cas-616e-to".to_string(), "to".to_string());
    let existing = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::Related);
    let pulled = Dependency::new(to.id.clone(), from.id.clone(), DependencyType::Blocks);
    let delete = team_dependency_fixture(&existing, project_id, Some("delete"));
    let upsert = team_dependency_fixture(&pulled, project_id, None);

    let (_temp, result, task_store, queue) = pull_team_task_and_dependency_fixtures(
        project_id,
        Vec::new(),
        vec![from.clone(), to.clone()],
        vec![existing],
        vec![delete, upsert],
    )
    .await;

    assert!(
        result.errors.is_empty(),
        "unexpected dependency pull errors: {:?}",
        result.errors
    );
    assert_eq!(result.pulled_task_dependencies, 1);
    assert!(task_store.get_dependencies(&from.id).unwrap().is_empty());
    assert_eq!(task_store.get_dependencies(&to.id).unwrap().len(), 1);
    assert_eq!(
        task_store.get_dependencies(&to.id).unwrap()[0].dep_type,
        DependencyType::Blocks
    );
    assert!(queue.pending(10, 5).unwrap().is_empty());
}

#[tokio::test]
async fn team_pull_parks_dangling_task_dependency_without_error() {
    use crate::types::{Dependency, DependencyType};

    let project_id = "dependency-project";
    let from = Task::new("cas-616e-dangling-from".to_string(), "from".to_string());
    let missing = Dependency::new(
        from.id.clone(),
        "cas-616e-missing".to_string(),
        DependencyType::Blocks,
    );
    let dangling = team_dependency_fixture(&missing, project_id, None);

    let (_temp, result, task_store, _queue) = pull_team_task_and_dependency_fixtures(
        project_id,
        Vec::new(),
        vec![from.clone()],
        Vec::new(),
        vec![dangling],
    )
    .await;

    assert!(
        result.errors.is_empty(),
        "dangling dependency should be parked: {:?}",
        result.errors
    );
    assert_eq!(result.pulled_task_dependencies, 0);
    assert!(task_store.get_dependencies(&from.id).unwrap().is_empty());
}

#[tokio::test]
async fn heal_local_task_dependency_enqueues_team_upsert() {
    use crate::types::{Dependency, DependencyType};

    let project_id = "dependency-heal-project";
    let from = Task::new("cas-heal-local-from".to_string(), "from".to_string());
    let to = Task::new("cas-heal-local-to".to_string(), "to".to_string());
    let local = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::Blocks);

    let (_temp, result, _task_store, queue) = pull_team_task_and_dependency_fixtures(
        project_id,
        Vec::new(),
        vec![from, to],
        vec![local],
        Vec::new(),
    )
    .await;

    assert_eq!(result.healed_task_dependencies_to_cloud, 1);
    assert_eq!(result.healed_task_dependencies_from_cloud, 0);
    let pending = queue
        .pending_for_team("team-cas-2125", 10, 5)
        .unwrap();
    assert_eq!(pending.len(), 1, "a local-only edge must be queued for team push");
    assert_eq!(pending[0].entity_id, "cas-heal-local-from:cas-heal-local-to:blocks");
    assert_eq!(pending[0].operation, crate::cloud::SyncOperation::Upsert);
}

#[tokio::test]
async fn heal_cloud_task_dependency_materializes_local_edge() {
    use crate::types::{Dependency, DependencyType};

    let project_id = "dependency-heal-project";
    let from = Task::new("cas-heal-cloud-from".to_string(), "from".to_string());
    let to = Task::new("cas-heal-cloud-to".to_string(), "to".to_string());
    let remote = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::Related);
    let remote_wire = team_dependency_fixture(&remote, project_id, None);

    let (_temp, result, task_store, queue) = pull_team_task_and_dependency_fixtures(
        project_id,
        Vec::new(),
        vec![from, to],
        Vec::new(),
        vec![remote_wire],
    )
    .await;

    assert_eq!(result.healed_task_dependencies_to_cloud, 0);
    assert_eq!(result.healed_task_dependencies_from_cloud, 1);
    let dependencies = task_store
        .get_dependencies("cas-heal-cloud-from")
        .unwrap();
    assert_eq!(dependencies.len(), 1);
    assert_eq!(dependencies[0].from_id, remote.from_id);
    assert_eq!(dependencies[0].to_id, remote.to_id);
    assert_eq!(dependencies[0].dep_type, remote.dep_type);
    assert!(queue.pending_for_team("team-cas-2125", 10, 5).unwrap().is_empty());
}

#[tokio::test]
async fn heal_task_dependencies_with_matching_sets_is_quiet() {
    use crate::types::{Dependency, DependencyType};

    let project_id = "dependency-heal-project";
    let from = Task::new("cas-heal-match-from".to_string(), "from".to_string());
    let to = Task::new("cas-heal-match-to".to_string(), "to".to_string());
    let matching = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::ParentChild);
    let remote_wire = team_dependency_fixture(&matching, project_id, None);

    let (_temp, result, task_store, queue) = pull_team_task_and_dependency_fixtures(
        project_id,
        Vec::new(),
        vec![from, to],
        vec![matching.clone()],
        vec![remote_wire],
    )
    .await;

    assert_eq!(result.healed_task_dependencies_to_cloud, 0);
    assert_eq!(result.healed_task_dependencies_from_cloud, 0);
    let dependencies = task_store
        .get_dependencies("cas-heal-match-from")
        .unwrap();
    assert_eq!(dependencies.len(), 1);
    assert_eq!(dependencies[0].from_id, matching.from_id);
    assert_eq!(dependencies[0].to_id, matching.to_id);
    assert_eq!(dependencies[0].dep_type, matching.dep_type);
    assert!(queue.pending_for_team("team-cas-2125", 10, 5).unwrap().is_empty());
}

#[tokio::test]
async fn heal_deleted_task_dependency_does_not_requeue_edge() {
    use crate::types::{Dependency, DependencyType};

    let project_id = "dependency-heal-project";
    let from = Task::new("cas-heal-delete-from".to_string(), "from".to_string());
    let to = Task::new("cas-heal-delete-to".to_string(), "to".to_string());
    let local = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::Blocks);
    let mut delete_wire = team_dependency_fixture(&local, project_id, Some("delete"));
    delete_wire
        .as_object_mut()
        .expect("dependency fixture is an object")
        .remove("created_at");
    let stale_upsert = team_dependency_fixture(&local, project_id, None);

    let (_temp, result, task_store, queue) = pull_team_task_and_dependency_fixtures(
        project_id,
        Vec::new(),
        vec![from, to],
        vec![local],
        vec![delete_wire, stale_upsert],
    )
    .await;

    assert_eq!(result.healed_task_dependencies_to_cloud, 0);
    assert_eq!(result.healed_task_dependencies_from_cloud, 0);
    assert!(task_store.get_dependencies("cas-heal-delete-from").unwrap().is_empty());
    assert!(queue.pending_for_team("team-cas-2125", 10, 5).unwrap().is_empty());
}

#[tokio::test]
async fn heal_local_task_dependency_is_idempotent_across_pulls() {
    use crate::types::{Dependency, DependencyType};

    let project_id = "dependency-heal-project";
    let from = Task::new("cas-heal-repeat-from".to_string(), "from".to_string());
    let to = Task::new("cas-heal-repeat-to".to_string(), "to".to_string());
    let local = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::Blocks);

    let (_temp, result, _task_store, queue) =
        pull_team_task_and_dependency_fixtures_with_pull_count(
            project_id,
            Vec::new(),
            vec![from, to],
            vec![local],
            Vec::new(),
            2,
        )
        .await;

    assert_eq!(result.healed_task_dependencies_to_cloud, 0);
    assert_eq!(result.healed_task_dependencies_from_cloud, 0);
    let pending = queue.pending_for_team("team-cas-2125", 10, 5).unwrap();
    assert_eq!(pending.len(), 1, "repeated pulls must not duplicate the queue row");
}

#[tokio::test]
async fn team_pull_duplicate_task_id_prefers_owner_closed_row() {
    let now = chrono::Utc::now();
    let tasks = vec![
        // The owner row is older and keyed by the owner project. The foreign
        // replica row is newer and active; wire order must not decide status.
        team_task_fixture(
            "cas-2125-owner-closed",
            TaskStatus::Closed,
            "owner-project",
            "owner-project",
            now - chrono::Duration::hours(1),
        ),
        team_task_fixture(
            "cas-2125-owner-closed",
            TaskStatus::Open,
            "replica-project",
            "owner-project",
            now,
        ),
    ];
    let (_temp, result, task_store, queue) =
        pull_team_task_fixtures("owner-project", tasks, None).await;

    assert!(
        result.errors.is_empty(),
        "unexpected pull errors: {:?}",
        result.errors
    );
    let task = task_store.get("cas-2125-owner-closed").unwrap();
    assert_eq!(task.status, TaskStatus::Closed);
    assert_eq!(task.origin_project.as_deref(), Some("owner-project"));
    let conflicts = queue.list_conflicts(10).unwrap();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].strategy, "owner_wins");
    assert!(conflicts[0].discarded_row_json.contains("replica-project"));
}

#[tokio::test]
async fn team_pull_single_foreign_open_row_is_parked_before_local_insert() {
    let task = team_task_fixture(
        "cas-2125-foreign-absent",
        TaskStatus::Open,
        "foreign-project",
        "foreign-project",
        chrono::Utc::now(),
    );
    let (_temp, result, task_store, _queue) =
        pull_team_task_and_dependency_fixtures_with_pull_count(
            "replica-project",
            vec![task],
            Vec::new(),
            Vec::new(),
            Vec::new(),
            2,
        )
        .await;

    assert!(
        result.errors.is_empty(),
        "unexpected pull errors: {:?}",
        result.errors
    );
    assert!(
        task_store.get("cas-2125-foreign-absent").is_err(),
        "foreign team task must be parked instead of becoming doctor contamination"
    );
}

#[tokio::test]
async fn team_pull_null_origin_uses_server_attested_project_identity() {
    let mut task = serde_json::to_value(Task::new(
        "cas-a1cf-null-origin".to_string(),
        "server-owned task".to_string(),
    ))
    .unwrap();
    task["origin_project"] = serde_json::Value::Null;
    task["project_id"] = serde_json::json!("server-project");

    let (_temp, result, task_store, _queue) =
        pull_team_task_fixtures("server-project", vec![task], None).await;

    assert!(result.errors.is_empty(), "unexpected pull errors: {:?}", result.errors);
    assert_eq!(
        task_store
            .get("cas-a1cf-null-origin")
            .unwrap()
            .origin_project
            .as_deref(),
        Some("server-project")
    );
}

#[tokio::test]
async fn team_pull_task_without_any_project_identity_is_parked() {
    let mut task = serde_json::to_value(Task::new(
        "cas-a1cf-no-identity".to_string(),
        "unattributed task".to_string(),
    ))
    .unwrap();
    task["origin_project"] = serde_json::Value::Null;
    task.as_object_mut().unwrap().remove("project_id");
    task.as_object_mut().unwrap().remove("project_canonical_id");

    let (_temp, result, task_store, _queue) =
        pull_team_task_fixtures("requested-project", vec![task], None).await;

    assert!(result.errors.is_empty(), "unexpected pull errors: {:?}", result.errors);
    assert!(
        task_store.get("cas-a1cf-no-identity").is_err(),
        "a task without an origin or server project must remain parked"
    );
}

#[tokio::test]
async fn team_pull_foreign_open_row_cannot_reopen_local_owner_close() {
    let now = chrono::Utc::now();
    let tasks = vec![team_task_fixture(
        "cas-2125-local-closed",
        TaskStatus::Open,
        "foreign-project",
        "foreign-project",
        now,
    )];
    let mut local = Task::new(
        "cas-2125-local-closed".to_string(),
        "owner closed task".to_string(),
    );
    local.status = TaskStatus::Closed;
    local.origin_project = Some("owner-project".to_string());
    local.closed_at = Some(now - chrono::Duration::hours(1));
    local.close_reason = Some("owner closed".to_string());
    local.updated_at = now - chrono::Duration::hours(1);
    let (_temp, result, task_store, _queue) =
        pull_team_task_fixtures("replica-project", tasks, Some(local)).await;

    assert!(
        result.errors.is_empty(),
        "unexpected pull errors: {:?}",
        result.errors
    );
    assert_eq!(
        task_store.get("cas-2125-local-closed").unwrap().status,
        TaskStatus::Closed
    );
}

// ---------------------------------------------------------------------------
// GH #640 client half (cas-cf1f): deletion tombstones and snapshot-scoped heal.
//
// The cloud's `task_dependencies` collection is filtered by `since` exactly
// like every other entity, so an incremental envelope is NOT a statement about
// which edges the cloud holds. Diffing the full local edge set against it made
// every untouched local edge look cloud-missing and re-queued it on every pull
// (measured on this host: 1,371 rows enqueued by a single pull). Reconciliation
// therefore runs only against a complete snapshot, and received tombstones are
// persisted so a later local push cannot resurrect a deleted edge.
// ---------------------------------------------------------------------------

const TOMBSTONE_TEAM: &str = "team-cas-cf1f";

fn dependency_tombstone_fixture(
    dependency: &crate::types::Dependency,
    project_id: &str,
    deleted_at: chrono::DateTime<chrono::Utc>,
) -> serde_json::Value {
    // Shape pinned by petra-stella-cloud PR #60: the live row is updated in
    // place, so a tombstone keeps from_id/to_id/dep_type/created_at and adds
    // `deleted`/`deleted_at` while `updated_at` becomes the delete time.
    let mut raw = team_dependency_fixture(dependency, project_id, None);
    raw["deleted"] = serde_json::json!(true);
    raw["deleted_at"] = serde_json::json!(deleted_at.to_rfc3339());
    raw["updated_at"] = serde_json::json!(deleted_at.to_rfc3339());
    raw
}

struct DependencyPullFixture {
    _temp: tempfile::TempDir,
    result: SyncResult,
    task_store: Arc<dyn TaskStore>,
    queue: Arc<SyncQueue>,
    _server: wiremock::MockServer,
}

#[allow(clippy::too_many_arguments)]
async fn pull_team_dependency_scenario(
    project_id: &str,
    local_tasks: Vec<Task>,
    local_dependencies: Vec<crate::types::Dependency>,
    envelope_edges: Vec<serde_json::Value>,
    snapshot_edges: Option<Vec<serde_json::Value>>,
    expected_snapshot_calls: u64,
    watermark: Option<&str>,
    reconciled_at: Option<&str>,
    ledger: Vec<(String, chrono::DateTime<chrono::Utc>)>,
) -> DependencyPullFixture {
    use crate::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig};
    use crate::store::{
        open_rule_store_local, open_skill_store_local, open_store_local, open_task_store_local,
    };
    use tempfile::TempDir;
    use wiremock::matchers::{method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    let envelope = ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "entries": [],
        "tasks": [],
        "rules": [],
        "skills": [],
        "task_dependencies": envelope_edges,
        "pulled_at": "2026-09-03T18:00:00Z",
        "team_id": TOMBSTONE_TEAM,
        "status": "ok",
    }));
    // The incremental envelope and the reconciliation snapshot are separate
    // requests with disjoint matchers, so a test can prove which one ran.
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TOMBSTONE_TEAM}/sync/pull")))
        .and(query_param("project_id", project_id))
        .and(query_param_is_missing("types"))
        .respond_with(envelope)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{TOMBSTONE_TEAM}/sync/pull")))
        .and(query_param("types", "task_dependencies"))
        .and(query_param_is_missing("since"))
        .respond_with(
            ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "task_dependencies": snapshot_edges.clone().unwrap_or_default(),
                "pulled_at": "2026-09-03T18:00:00Z",
                "team_id": TOMBSTONE_TEAM,
                "status": "ok",
            })),
        )
        .expect(expected_snapshot_calls)
        .mount(&server)
        .await;

    let temp = TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
    queue.init().unwrap();
    if let Some(watermark) = watermark {
        queue
            .set_metadata(
                &format!("last_team_pull_at_{TOMBSTONE_TEAM}_{project_id}"),
                watermark,
            )
            .unwrap();
    }
    if let Some(reconciled_at) = reconciled_at {
        queue
            .set_metadata(
                &format!("last_dependency_reconcile_at_{TOMBSTONE_TEAM}_{project_id}"),
                reconciled_at,
            )
            .unwrap();
    }
    let store = open_store_local(temp.path()).unwrap();
    let task_store = open_task_store_local(temp.path()).unwrap();
    for task in local_tasks {
        task_store.add(&task).unwrap();
    }
    for dependency in &local_dependencies {
        task_store.add_dependency(dependency).unwrap();
    }
    for (entity_id, deleted_at) in ledger {
        let mut parts = entity_id.splitn(3, ':');
        let from_id = parts.next().unwrap_or_default().to_string();
        let to_id = parts.next().unwrap_or_default().to_string();
        let dep_type = parts.next().unwrap_or_default().to_string();
        queue
            .record_dependency_tombstone(&entity_id, &from_id, &to_id, &dep_type, deleted_at)
            .unwrap();
    }
    let rule_store = open_rule_store_local(temp.path()).unwrap();
    let skill_store = open_skill_store_local(temp.path()).unwrap();
    let syncer = CloudSyncer::new(
        Arc::clone(&queue),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
    );
    let result = syncer
        .pull_team(
            TOMBSTONE_TEAM,
            project_id,
            store.as_ref(),
            task_store.as_ref(),
            rule_store.as_ref(),
            skill_store.as_ref(),
        )
        .unwrap();

    DependencyPullFixture {
        _temp: temp,
        result,
        task_store,
        queue,
        _server: server,
    }
}

#[tokio::test]
async fn pulled_tombstone_deletes_the_local_edge_and_pins_it() {
    use crate::types::{Dependency, DependencyType};

    let project_id = "tombstone-project";
    let from = Task::new("cas-cf1f-from".to_string(), "from".to_string());
    let to = Task::new("cas-cf1f-to".to_string(), "to".to_string());
    let mut local = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::ParentChild);
    local.created_at = chrono::Utc::now() - chrono::Duration::hours(4);
    let deleted_at = chrono::Utc::now() - chrono::Duration::hours(1);
    let tombstone = dependency_tombstone_fixture(&local, project_id, deleted_at);

    let fixture = pull_team_dependency_scenario(
        project_id,
        vec![from.clone(), to],
        vec![local.clone()],
        vec![tombstone],
        None,
        0,
        None,
        None,
        Vec::new(),
    )
    .await;

    assert!(
        fixture.result.errors.is_empty(),
        "unexpected errors: {:?}",
        fixture.result.errors
    );
    assert_eq!(fixture.result.deleted_task_dependencies, 1);
    assert!(
        fixture
            .task_store
            .get_dependencies(&from.id)
            .unwrap()
            .is_empty(),
        "the tombstoned edge must be gone locally"
    );
    let recorded = fixture
        .queue
        .dependency_tombstone("cas-cf1f-from:cas-cf1f-to:parent-child")
        .unwrap()
        .expect("the tombstone must be persisted so a later push cannot resurrect the edge");
    assert_eq!(recorded.timestamp(), deleted_at.timestamp());
    assert!(
        fixture
            .queue
            .pending_for_team(TOMBSTONE_TEAM, 10, 5)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn heal_never_repushes_an_edge_a_recorded_tombstone_deleted() {
    use crate::types::{Dependency, DependencyType};

    let project_id = "tombstone-project";
    let from = Task::new("cas-cf1f-stale-from".to_string(), "from".to_string());
    let to = Task::new("cas-cf1f-stale-to".to_string(), "to".to_string());
    let mut local = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::Blocks);
    local.created_at = chrono::Utc::now() - chrono::Duration::days(2);
    let entity_id = "cas-cf1f-stale-from:cas-cf1f-stale-to:blocks".to_string();

    let fixture = pull_team_dependency_scenario(
        project_id,
        vec![from.clone(), to],
        vec![local],
        Vec::new(),
        None,
        0,
        None,
        None,
        vec![(entity_id, chrono::Utc::now() - chrono::Duration::hours(6))],
    )
    .await;

    assert_eq!(fixture.result.healed_task_dependencies_to_cloud, 0);
    assert_eq!(fixture.result.skipped_task_dependencies_by_tombstone, 1);
    assert!(
        fixture
            .queue
            .pending_for_team(TOMBSTONE_TEAM, 10, 5)
            .unwrap()
            .is_empty(),
        "a tombstoned edge must never be queued back to the cloud"
    );
    assert!(
        fixture
            .task_store
            .get_dependencies(&from.id)
            .unwrap()
            .is_empty(),
        "the local edge converges to deleted rather than lingering unsynced"
    );
}

#[tokio::test]
async fn heal_pushes_an_edge_recreated_after_its_tombstone() {
    use crate::types::{Dependency, DependencyType};

    let project_id = "tombstone-project";
    let from = Task::new("cas-cf1f-readd-from".to_string(), "from".to_string());
    let to = Task::new("cas-cf1f-readd-to".to_string(), "to".to_string());
    let mut local = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::Blocks);
    local.created_at = chrono::Utc::now() - chrono::Duration::minutes(5);
    let entity_id = "cas-cf1f-readd-from:cas-cf1f-readd-to:blocks".to_string();

    let fixture = pull_team_dependency_scenario(
        project_id,
        vec![from.clone(), to],
        vec![local],
        Vec::new(),
        None,
        0,
        None,
        None,
        vec![(
            entity_id.clone(),
            chrono::Utc::now() - chrono::Duration::hours(3),
        )],
    )
    .await;

    assert_eq!(fixture.result.healed_task_dependencies_to_cloud, 1);
    assert_eq!(fixture.result.skipped_task_dependencies_by_tombstone, 0);
    let pending = fixture.queue.pending_for_team(TOMBSTONE_TEAM, 10, 5).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].entity_id, entity_id);
    assert!(
        fixture.queue.dependency_tombstone(&entity_id).unwrap().is_none(),
        "a newer local edge retires its tombstone"
    );
    assert_eq!(
        fixture.task_store.get_dependencies(&from.id).unwrap().len(),
        1
    );
}

#[tokio::test]
async fn incremental_pull_does_not_reheal_edges_outside_the_since_window() {
    use crate::types::{Dependency, DependencyType};

    let project_id = "tombstone-project";
    let from = Task::new("cas-cf1f-churn-from".to_string(), "from".to_string());
    let to = Task::new("cas-cf1f-churn-to".to_string(), "to".to_string());
    let local = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::ParentChild);

    // Watermarked pull, reconciliation already done minutes ago: the partial
    // envelope must not be mistaken for the cloud's full edge set.
    let fixture = pull_team_dependency_scenario(
        project_id,
        vec![from, to],
        vec![local],
        Vec::new(),
        Some(Vec::new()),
        0,
        Some("2026-09-03T17:48:41.321Z"),
        Some(&(chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339()),
        Vec::new(),
    )
    .await;

    assert_eq!(fixture.result.healed_task_dependencies_to_cloud, 0);
    assert!(
        fixture
            .queue
            .pending_for_team(TOMBSTONE_TEAM, 10, 5)
            .unwrap()
            .is_empty(),
        "an incremental envelope must never re-queue the local edge set"
    );
}

#[tokio::test]
async fn incremental_pull_reconciles_against_the_full_snapshot_when_due() {
    use crate::types::{Dependency, DependencyType};

    let project_id = "tombstone-project";
    let from = Task::new("cas-cf1f-due-from".to_string(), "from".to_string());
    let to = Task::new("cas-cf1f-due-to".to_string(), "to".to_string());
    let local = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::Blocks);

    let fixture = pull_team_dependency_scenario(
        project_id,
        vec![from, to],
        vec![local],
        Vec::new(),
        Some(Vec::new()),
        1,
        Some("2026-09-03T17:48:41.321Z"),
        Some(&(chrono::Utc::now() - chrono::Duration::days(1)).to_rfc3339()),
        Vec::new(),
    )
    .await;

    assert_eq!(fixture.result.healed_task_dependencies_to_cloud, 1);
    let pending = fixture.queue.pending_for_team(TOMBSTONE_TEAM, 10, 5).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].entity_id, "cas-cf1f-due-from:cas-cf1f-due-to:blocks");
}

#[tokio::test]
async fn reconciliation_snapshot_tombstone_deletes_the_local_edge() {
    use crate::types::{Dependency, DependencyType};

    let project_id = "tombstone-project";
    let from = Task::new("cas-cf1f-snap-from".to_string(), "from".to_string());
    let to = Task::new("cas-cf1f-snap-to".to_string(), "to".to_string());
    let mut local = Dependency::new(from.id.clone(), to.id.clone(), DependencyType::Blocks);
    local.created_at = chrono::Utc::now() - chrono::Duration::days(1);
    let tombstone = dependency_tombstone_fixture(
        &local,
        project_id,
        chrono::Utc::now() - chrono::Duration::hours(2),
    );

    let fixture = pull_team_dependency_scenario(
        project_id,
        vec![from.clone(), to],
        vec![local],
        Vec::new(),
        Some(vec![tombstone]),
        1,
        Some("2026-09-03T17:48:41.321Z"),
        None,
        Vec::new(),
    )
    .await;

    assert_eq!(fixture.result.deleted_task_dependencies, 1);
    assert_eq!(fixture.result.healed_task_dependencies_to_cloud, 0);
    assert!(
        fixture
            .task_store
            .get_dependencies(&from.id)
            .unwrap()
            .is_empty()
    );
    assert!(
        fixture
            .queue
            .dependency_tombstone("cas-cf1f-snap-from:cas-cf1f-snap-to:blocks")
            .unwrap()
            .is_some()
    );
}

/// GH #668: the deployed cloud returns per-row verdicts in a TOP-LEVEL `rows`
/// array (verified against /api/sync/push on 2026-09-03). A row the cloud kept
/// a newer version of leaves the queue and is reported as an acknowledgement,
/// not as a failure.
#[tokio::test]
async fn top_level_rows_ack_lww_skips_and_park_rejections_by_reason() {
    use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": {"inserted": 1, "updated": 0, "skipped": 2},
            "rows": [
                {"entity_type": "entries", "id": "entry-written", "outcome": "inserted"},
                {"entity_type": "entries", "id": "entry-kept-newer", "outcome": "skipped_lww"},
                {
                    "entity_type": "entries",
                    "id": "entry-refused",
                    "outcome": "rejected",
                    "reason": "project_mismatch"
                }
            ],
            "canonical_id": "test-project"
        })))
        .mount(&server)
        .await;

    let temp = tempdir().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
    queue.init().unwrap();
    for id in ["entry-written", "entry-kept-newer", "entry-refused"] {
        queue
            .enqueue(
                EntityType::Entry,
                id,
                SyncOperation::Upsert,
                Some(&format!(r#"{{"id":"{id}"}}"#)),
            )
            .unwrap();
    }

    let syncer = CloudSyncer::new_for_project(
        queue.clone(),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
        "test-project".to_string(),
        temp.path(),
    );
    let result = syncer.push_scoped(PushScope::EntriesOnly).unwrap();

    assert_eq!(
        result.skipped_lww_acked, 1,
        "the LWW loss is an acknowledgement"
    );
    let remaining = queue.list_all(10).unwrap();
    assert_eq!(remaining.len(), 1, "only the refused row is retained");
    assert_eq!(remaining[0].entity_id, "entry-refused");
    assert_eq!(
        result
            .remaining_backlog
            .rejected_by_reason
            .get("project_mismatch")
            .copied(),
        Some(1),
        "the cloud's reason is persisted for reporting"
    );
    assert!(
        result.remaining_backlog.failed_errors[0].contains("cas cloud link"),
        "the diagnostic carries the remediation for its reason: {:?}",
        result.remaining_backlog.failed_errors
    );
}

/// A cloud build that answers with aggregate counts only must behave exactly as
/// it did before per-row results existed.
#[tokio::test]
async fn responses_without_rows_keep_the_legacy_aggregate_behaviour() {
    use crate::cloud::{CloudConfig, EntityType, SyncOperation, SyncQueue};
    use tempfile::tempdir;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": {"inserted": 1, "updated": 0, "skipped": 1}
        })))
        .mount(&server)
        .await;

    let temp = tempdir().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
    queue.init().unwrap();
    for id in ["entry-one", "entry-two"] {
        queue
            .enqueue(
                EntityType::Entry,
                id,
                SyncOperation::Upsert,
                Some(&format!(r#"{{"id":"{id}"}}"#)),
            )
            .unwrap();
    }

    let syncer = CloudSyncer::new_for_project(
        queue.clone(),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
        "test-project".to_string(),
        temp.path(),
    );
    let result = syncer.push_scoped(PushScope::EntriesOnly).unwrap();

    assert!(result.errors.is_empty());
    assert_eq!(result.pushed_entries, 2);
    assert_eq!(
        result.skipped_lww_acked, 1,
        "an aggregate skip is still reported as a kept-newer row"
    );
    assert!(queue.list_all(10).unwrap().is_empty());
    assert!(result.remaining_backlog.rejected_by_reason.is_empty());
}

/// Every reason the deployed cloud can return maps to a distinct, actionable
/// remediation; an unknown reason says so instead of inventing a fix.
#[test]
fn every_push_reason_has_its_own_remediation() {
    for reason in [
        "project_mismatch",
        "scope_mismatch",
        "revision_conflict",
        "version_gate",
        "sync_limit_exceeded",
    ] {
        let hint = push_reason_hint(reason);
        assert!(
            !hint.starts_with("unrecognized"),
            "{reason} must name its own repair"
        );
    }
    assert!(push_reason_hint("brand_new_server_reason").starts_with("unrecognized"));
    assert!(push_reason_is_permanent("project_mismatch"));
    assert!(push_reason_is_permanent("SCOPE_MISMATCH"));
    assert!(!push_reason_is_permanent("revision_conflict"));
}

// ---------------------------------------------------------------------------
// cas-c32f: revision-based conflict resolution.
//
// The cloud owns a monotonic per-row `revision` and increments it on every
// accepted write, so it is the only trustworthy answer to "which side is
// newer". Comparing it BEFORE `updated_at` is what stops a machine with a
// wrong clock from silently winning or losing a conflict. When either side has
// no revision the timestamp path must behave exactly as it always did.
// ---------------------------------------------------------------------------

fn revision_syncer() -> (tempfile::TempDir, CloudSyncer) {
    use crate::cloud::{CloudConfig, CloudSyncerConfig};

    let temp = tempfile::TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
    queue.init().unwrap();
    let syncer = CloudSyncer::new(
        queue,
        CloudConfig {
            endpoint: "https://cloud.invalid".to_string(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
    );
    (temp, syncer)
}

#[tokio::test]
async fn a_slow_clock_cannot_lose_a_conflict_it_wins_on_revision() {
    let (_temp, syncer) = revision_syncer();
    let now = chrono::Utc::now();
    // Local machine's clock is an hour behind, but it holds the newer server
    // revision. Under pure timestamp LWW the remote row would win and the
    // local edit would be silently discarded.
    let action = syncer.resolve_conflict_with_revisions(
        "task",
        "cas-c32f-slow-clock",
        now - chrono::Duration::hours(1),
        now,
        Some(9),
        Some(8),
        ConflictResolution::KeepRecent,
    );
    assert_eq!(action, ConflictAction::UseLocal);

    let conflicts = syncer.take_conflict_log();
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].local_revision, Some(9));
    assert_eq!(conflicts[0].remote_revision, Some(8));
    assert!(conflicts[0].decided_by_revision());
    assert_eq!(conflicts[0].strategy_label(), "revision");
}

#[tokio::test]
async fn a_fast_clock_cannot_win_a_conflict_it_loses_on_revision() {
    let (_temp, syncer) = revision_syncer();
    let now = chrono::Utc::now();
    // The mirror image: the local clock runs an hour ahead, but the remote row
    // carries the newer revision and must win anyway.
    let action = syncer.resolve_conflict_with_revisions(
        "task",
        "cas-c32f-fast-clock",
        now + chrono::Duration::hours(1),
        now,
        Some(3),
        Some(4),
        ConflictResolution::KeepRecent,
    );
    assert_eq!(action, ConflictAction::UseRemote);
    assert!(syncer.take_conflict_log()[0].decided_by_revision());
}

#[tokio::test]
async fn revision_ten_beats_revision_nine() {
    // Guards the wire format: `revision` arrives as a decimal STRING, so a
    // lexicographic comparison would rank "9" above "10" and invert every
    // conflict from a row's tenth edit onward.
    let (_temp, syncer) = revision_syncer();
    let now = chrono::Utc::now();
    let action = syncer.resolve_conflict_with_revisions(
        "task",
        "cas-c32f-ten",
        now,
        now - chrono::Duration::minutes(1),
        crate::cloud::parse_wire_revision(Some(&serde_json::json!("9"))),
        crate::cloud::parse_wire_revision(Some(&serde_json::json!("10"))),
        ConflictResolution::KeepRecent,
    );
    assert_eq!(action, ConflictAction::UseRemote);
}

#[tokio::test]
async fn equal_revisions_fall_through_to_the_timestamp_comparison() {
    // Equal revisions mean both sides descend from the same server state, so
    // an unpushed local edit must still be recognisable by its timestamp.
    let (_temp, syncer) = revision_syncer();
    let now = chrono::Utc::now();
    let action = syncer.resolve_conflict_with_revisions(
        "task",
        "cas-c32f-equal",
        now,
        now - chrono::Duration::minutes(5),
        Some(4),
        Some(4),
        ConflictResolution::KeepRecent,
    );
    assert_eq!(action, ConflictAction::UseLocal);
    let conflict = &syncer.take_conflict_log()[0];
    assert!(!conflict.decided_by_revision());
    assert_eq!(conflict.strategy_label(), "timestamp_lww");
}

#[tokio::test]
async fn a_missing_revision_on_either_side_keeps_the_timestamp_path_unchanged() {
    let (_temp, syncer) = revision_syncer();
    let now = chrono::Utc::now();
    let older = now - chrono::Duration::hours(1);

    // Backward compatibility: a row this client has never pulled, or a cloud
    // build that does not send revisions, must resolve exactly as before —
    // including when the side WITHOUT a revision is the one that wins.
    for (local_revision, remote_revision) in
        [(None, None), (Some(9), None), (None, Some(9))]
    {
        assert_eq!(
            syncer.resolve_conflict_with_revisions(
                "task",
                "cas-c32f-compat",
                older,
                now,
                local_revision,
                remote_revision,
                ConflictResolution::KeepRecent,
            ),
            ConflictAction::UseRemote,
            "newer remote timestamp must win when revisions are incomplete"
        );
        assert_eq!(
            syncer.resolve_conflict_with_revisions(
                "task",
                "cas-c32f-compat",
                now,
                older,
                local_revision,
                remote_revision,
                ConflictResolution::KeepRecent,
            ),
            ConflictAction::UseLocal,
            "newer local timestamp must win when revisions are incomplete"
        );
        assert_eq!(
            syncer.resolve_conflict_with_revisions(
                "task",
                "cas-c32f-compat",
                now,
                now,
                local_revision,
                remote_revision,
                ConflictResolution::KeepRecent,
            ),
            ConflictAction::Skip,
            "identical timestamps must still skip the write"
        );
    }
}

#[tokio::test]
async fn an_explicit_operator_strategy_outranks_the_revisions() {
    // RemoteWins/LocalWins are deliberate operator choices, not a guess about
    // which row is newer, so revisions must not override them.
    let (_temp, syncer) = revision_syncer();
    let now = chrono::Utc::now();
    assert_eq!(
        syncer.resolve_conflict_with_revisions(
            "task",
            "cas-c32f-operator",
            now,
            now,
            Some(99),
            Some(1),
            ConflictResolution::RemoteWins,
        ),
        ConflictAction::UseRemote
    );
    assert_eq!(
        syncer.resolve_conflict_with_revisions(
            "task",
            "cas-c32f-operator",
            now,
            now,
            Some(1),
            Some(99),
            ConflictResolution::LocalWins,
        ),
        ConflictAction::UseLocal
    );
    for conflict in syncer.take_conflict_log() {
        assert!(!conflict.decided_by_revision());
    }
}

#[tokio::test]
async fn the_revision_ledger_is_monotonic_and_scoped_per_entity_type() {
    use crate::cloud::EntityType;

    let (_temp, syncer) = revision_syncer();
    let queue = syncer.queue();
    queue.record_revision(EntityType::Task, "cas-c32f-led", 5).unwrap();
    // A replayed older envelope must not roll the ledger backwards: the base
    // revision we send later is what decides whether our push is accepted.
    queue.record_revision(EntityType::Task, "cas-c32f-led", 3).unwrap();
    assert_eq!(queue.revision(EntityType::Task, "cas-c32f-led").unwrap(), Some(5));
    queue.record_revision(EntityType::Task, "cas-c32f-led", 6).unwrap();
    assert_eq!(queue.revision(EntityType::Task, "cas-c32f-led").unwrap(), Some(6));

    // The same id under another entity type is an independent row.
    assert_eq!(queue.revision(EntityType::Entry, "cas-c32f-led").unwrap(), None);
    queue.record_revision(EntityType::Entry, "cas-c32f-led", 2).unwrap();
    assert_eq!(queue.revision(EntityType::Entry, "cas-c32f-led").unwrap(), Some(2));
    assert_eq!(queue.revision(EntityType::Task, "cas-c32f-led").unwrap(), Some(6));

    queue.clear_revision(EntityType::Task, "cas-c32f-led").unwrap();
    assert_eq!(queue.revision(EntityType::Task, "cas-c32f-led").unwrap(), None);
}

// --- cas-c32f milestone 2: the base revision on the wire, and the receipts ---

fn decode_push_body(request: &wiremock::Request) -> serde_json::Value {
    use flate2::read::GzDecoder;
    use std::io::Read;

    let mut decoder = GzDecoder::new(request.body.as_slice());
    let mut decoded = Vec::new();
    if decoder.read_to_end(&mut decoded).is_ok() {
        if let Ok(value) = serde_json::from_slice(&decoded) {
            return value;
        }
    }
    serde_json::from_slice(&request.body).unwrap_or(serde_json::Value::Null)
}

#[test]
fn accepted_and_conflicting_revisions_are_read_from_either_envelope() {
    // Personal puts per-type keys at the top level; team nests them under
    // `synced`. Both must yield the same receipts, or team pushes would
    // silently never learn a revision.
    let personal: PushResponse = serde_json::from_value(serde_json::json!({
        "tasks": {
            "inserted": 1, "updated": 0, "skipped": 1,
            "accepted": {"cas-ok": {"revision": "6", "canonical_id": "cas-src"}},
            "rejected": [{
                "id": "cas-stale", "reason": "revision_conflict",
                "existing_canonical_id": "cas-src", "current_revision": "9"
            }]
        },
        "rows": [
            {"entity_type": "tasks", "id": "cas-ok", "outcome": "updated"},
            {"entity_type": "tasks", "id": "cas-stale", "outcome": "rejected", "reason": "revision_conflict"}
        ]
    }))
    .unwrap();
    let team: PushResponse = serde_json::from_value(serde_json::json!({
        "synced": {
            "tasks": {
                "inserted": 1, "updated": 0, "skipped": 1,
                "accepted": {"cas-ok": {"revision": "6", "canonical_id": "cas-src"}},
                "rejected": [{
                    "id": "cas-stale", "reason": "revision_conflict",
                    "existing_canonical_id": "cas-src", "current_revision": "9"
                }]
            }
        }
    }))
    .unwrap();

    for response in [&personal, &team] {
        assert_eq!(
            response.accepted_revisions_for("tasks").get("cas-ok"),
            Some(&6)
        );
        assert_eq!(
            response.revision_conflicts_for("tasks").get("cas-stale"),
            Some(&Some(9))
        );
        // A rejection for another reason is not a revision conflict.
        assert!(!response.revision_conflicts_for("tasks").contains_key("cas-ok"));
    }
}

#[test]
fn a_stale_base_is_retryable_rather_than_parked() {
    // Losing the race is a stale base, not a bad row: parking it would strand
    // the user's edit behind a manual requeue.
    let conflict = PushRowResult {
        id: "cas-stale".to_string(),
        outcome: PushRowOutcome::Rejected,
        reason: Some("revision_conflict".to_string()),
    };
    assert!(conflict.rejection_is_retryable());
    // An unknown reason still parks — losing a diagnostic row is worse.
    let unknown = PushRowResult {
        id: "cas-unknown".to_string(),
        outcome: PushRowOutcome::Rejected,
        reason: Some("something_new".to_string()),
    };
    assert!(!unknown.rejection_is_retryable());
}

#[tokio::test]
async fn push_declares_the_stored_base_revision_and_omits_it_when_unknown() {
    use crate::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig, EntityType};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "tasks": {
                "inserted": 0, "updated": 2, "skipped": 0,
                "accepted": {
                    "cas-c32f-known": {"revision": "8", "canonical_id": "p"},
                    "cas-c32f-new": {"revision": "1", "canonical_id": "p"}
                }
            },
            "rows": [
                {"entity_type": "tasks", "id": "cas-c32f-known", "outcome": "updated"},
                {"entity_type": "tasks", "id": "cas-c32f-new", "outcome": "inserted"}
            ]
        })))
        .mount(&server)
        .await;

    let temp = tempfile::TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
    queue.init().unwrap();
    // One row whose revision we have observed, one we have never pulled.
    queue.record_revision(EntityType::Task, "cas-c32f-known", 7).unwrap();
    for id in ["cas-c32f-known", "cas-c32f-new"] {
        queue
            .enqueue(
                EntityType::Task,
                id,
                crate::cloud::SyncOperation::Upsert,
                Some(&serde_json::json!({"id": id, "title": "t"}).to_string()),
            )
            .unwrap();
    }

    let syncer = CloudSyncer::new_for_project(
        Arc::clone(&queue),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
        "p".to_string(),
        temp.path(),
    );
    tokio::task::spawn_blocking(move || syncer.push())
        .await
        .unwrap()
        .unwrap();

    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = requests
        .iter()
        .find_map(|request| {
            let decoded = decode_push_body(request);
            decoded
                .get("tasks")
                .is_some_and(|tasks| !tasks.as_array().unwrap_or(&vec![]).is_empty())
                .then_some(decoded)
        })
        .expect("a task push must have been sent");
    let tasks = body["tasks"].as_array().unwrap();
    let known = tasks
        .iter()
        .find(|task| task["id"] == "cas-c32f-known")
        .unwrap();
    let fresh = tasks
        .iter()
        .find(|task| task["id"] == "cas-c32f-new")
        .unwrap();
    // Known row declares its base as a decimal STRING; unknown row omits the
    // key entirely, which is what selects the server's timestamp path. A
    // placeholder would be dropped by the server as an unparseable revision.
    assert_eq!(known["revision"], serde_json::json!("7"));
    assert!(
        fresh.get("revision").is_none(),
        "a row with no observed revision must omit the key, not send a placeholder"
    );

    // The echoed revisions are stored, so the next push declares a base the
    // server will accept without a re-pull first.
    assert_eq!(queue.revision(EntityType::Task, "cas-c32f-known").unwrap(), Some(8));
    assert_eq!(queue.revision(EntityType::Task, "cas-c32f-new").unwrap(), Some(1));
}

#[tokio::test]
async fn a_rejected_stale_base_is_forgotten_rather_than_replaced() {
    use crate::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig, EntityType};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/sync/push"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "tasks": {
                "inserted": 0, "updated": 0, "skipped": 1,
                "rejected": [{
                    "id": "cas-c32f-lost", "reason": "revision_conflict",
                    "existing_canonical_id": "p", "current_revision": "12"
                }]
            },
            "rows": [{
                "entity_type": "tasks", "id": "cas-c32f-lost",
                "outcome": "rejected", "reason": "revision_conflict"
            }]
        })))
        .mount(&server)
        .await;

    let temp = tempfile::TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
    queue.init().unwrap();
    queue.record_revision(EntityType::Task, "cas-c32f-lost", 4).unwrap();
    queue
        .enqueue(
            EntityType::Task,
            "cas-c32f-lost",
            crate::cloud::SyncOperation::Upsert,
            Some(&serde_json::json!({"id": "cas-c32f-lost", "title": "t"}).to_string()),
        )
        .unwrap();

    let syncer = CloudSyncer::new_for_project(
        Arc::clone(&queue),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig::default(),
        "p".to_string(),
        temp.path(),
    );
    let _ = tokio::task::spawn_blocking(move || syncer.push()).await.unwrap();

    // The proven-stale base is dropped, NOT replaced with the server's current
    // revision: adopting a revision whose body we have never seen would let the
    // next push overwrite a change this machine never looked at.
    assert_eq!(queue.revision(EntityType::Task, "cas-c32f-lost").unwrap(), None);
    // The edit itself stays queued for the next cycle rather than being parked.
    assert_eq!(queue.pending(10, 5).unwrap().len(), 1);
}

/// The comparison has to fire on a REAL pull, not only through the unit-level
/// API: the revision travels on the raw wire row while the decision is taken
/// deep inside a typed upsert helper. This drives a whole team pull to prove
/// the two ends are actually connected.
#[tokio::test]
async fn a_real_pull_lets_a_higher_local_revision_beat_a_newer_remote_timestamp() {
    use crate::cloud::{CloudConfig, CloudSyncer, CloudSyncerConfig, EntityType, SyncOperation};
    use crate::store::{
        open_rule_store_local, open_skill_store_local, open_store_local, open_task_store_local,
    };
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let project_id = "revision-pull-project";
    let team_id = "team-cas-c32f";
    let now = chrono::Utc::now();

    // The cloud row is an hour NEWER by the clock but a revision BEHIND: this
    // is the skewed-clock case, and the local row must survive it.
    let mut remote = Task::new("cas-c32f-pull".to_string(), "remote title".to_string());
    remote.updated_at = now + chrono::Duration::hours(1);
    remote.origin_project = Some(project_id.to_string());
    let mut wire = serde_json::to_value(&remote).unwrap();
    wire["project_id"] = serde_json::json!(project_id);
    wire["revision"] = serde_json::json!("4");

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/api/teams/{team_id}/sync/pull")))
        .and(query_param("project_id", project_id))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "entries": [], "tasks": [wire], "rules": [], "skills": [],
            "task_dependencies": [],
            "pulled_at": "2026-09-03T22:00:00Z",
            "team_id": team_id,
            "status": "ok",
        })))
        .mount(&server)
        .await;

    let temp = tempfile::TempDir::new().unwrap();
    // Pin the scratch root: the ephemeral-project guard refuses an unpinned
    // root under the temp directory, and a TempDir is exactly that.
    std::fs::write(temp.path().join("config.toml"), "[project]\ncanonical_id = \"p\"\n").unwrap();
    let queue = Arc::new(SyncQueue::open(temp.path()).unwrap());
    queue.init().unwrap();
    let store = open_store_local(temp.path()).unwrap();
    let task_store = open_task_store_local(temp.path()).unwrap();
    let rule_store = open_rule_store_local(temp.path()).unwrap();
    let skill_store = open_skill_store_local(temp.path()).unwrap();

    let mut local = Task::new("cas-c32f-pull".to_string(), "local title".to_string());
    local.updated_at = now;
    local.origin_project = Some(project_id.to_string());
    task_store.add(&local).unwrap();
    // This machine holds revision 7; the incoming row is revision 4.
    queue.record_revision(EntityType::Task, "cas-c32f-pull", 7).unwrap();
    // A pending local change makes the discarded-row journal fire.
    queue
        .enqueue(
            EntityType::Task,
            "cas-c32f-pull",
            SyncOperation::Upsert,
            Some("{}"),
        )
        .unwrap();

    let syncer = CloudSyncer::new(
        Arc::clone(&queue),
        CloudConfig {
            endpoint: server.uri(),
            token: Some("test-token".to_string()),
            ..Default::default()
        },
        CloudSyncerConfig {
            // "keep whichever is newer" is the question revisions arbitrate.
            // The default team strategy is RemoteWins, a deliberate operator
            // choice that revisions must NOT override.
            team_conflict_resolution: ConflictResolution::KeepRecent,
            ..CloudSyncerConfig::default()
        },
    );
    syncer
        .pull_team(
            team_id,
            project_id,
            store.as_ref(),
            task_store.as_ref(),
            rule_store.as_ref(),
            skill_store.as_ref(),
        )
        .unwrap();

    assert_eq!(
        task_store.get("cas-c32f-pull").unwrap().title,
        "local title",
        "a newer remote TIMESTAMP must not overwrite a higher local REVISION"
    );
}
