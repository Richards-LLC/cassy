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
fn push_response_is_backward_compatible_with_legacy_payload() {
    // Older cloud builds may return shapes like {"synced": {...}} or just
    // an empty body. Either must deserialize into a PushResponse whose
    // skipped field is None, so the client falls back to legacy
    // mark-synced behavior rather than treating the absence as "all
    // skipped".
    let legacy_synced_shape =
        r#"{"synced":{"entries":5,"tasks":0,"rules":0,"skills":0,"sessions":0,
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
