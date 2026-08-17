use std::time::Duration;

use crate::cloud::syncer::*;

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
        result.pushed_entries, 0,
        "the batch reports its queue error"
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
