use crate::types::*;

#[test]
fn coordination_request_rejects_unknown_fields() {
    let error = serde_json::from_str::<CoordinationRequest>(
        r#"{"action":"shutdown_workers","worker_namse":"alice"}"#,
    )
    .expect_err("unknown fields on destructive requests must not be discarded");
    let message = error.to_string();
    assert!(
        message.contains("worker_namse") || message.contains("unknown field"),
        "unexpected serde error: {message}"
    );
}

#[test]
fn test_team_request_default() {
    let req: TeamRequest = serde_json::from_str(r#"{"action": "list"}"#).unwrap();
    assert_eq!(req.action, "list");
    assert!(req.team_id.is_none());
    assert!(req.limit.is_none());
}

#[test]
fn test_memory_request_default() {
    let req: MemoryRequest = serde_json::from_str(r#"{"action": "list"}"#).unwrap();
    assert_eq!(req.action, "list");
    assert!(req.id.is_none());
    assert!(req.content.is_none());
}

#[test]
fn test_task_request_with_options() {
    let req: TaskRequest = serde_json::from_str(
        r#"{
            "action": "create",
            "title": "Test task",
            "priority": 1
        }"#,
    )
    .unwrap();
    assert_eq!(req.action, "create");
    assert_eq!(req.title, Some("Test task".to_string()));
    assert_eq!(req.priority, Some(1));
}

#[test]
fn close_request_rejects_removed_findings_field() {
    let error = serde_json::from_str::<TaskRequest>(
        r#"{"action":"close","id":"cas-close","code_review_findings":"{}"}"#,
    )
    .expect_err("removed review envelope must be a request-schema error");
    assert!(error.to_string().contains("code_review_findings"));
}

#[test]
fn close_request_maps_deprecated_bypass_alias_to_supervisor_override() {
    let request: TaskRequest =
        serde_json::from_str(r#"{"action":"close","id":"cas-close","bypass_code_review":true}"#)
            .unwrap();
    assert_eq!(request.effective_supervisor_override(), Some(true));
    assert!(request.used_deprecated_supervisor_override_alias());
}

#[test]
fn test_system_request_reindex() {
    let req: SystemRequest = serde_json::from_str(
        r#"{
            "action": "reindex",
            "bm25": true,
            "embeddings": false
        }"#,
    )
    .unwrap();
    assert_eq!(req.action, "reindex");
    assert_eq!(req.bm25, Some(true));
    assert_eq!(req.embeddings, Some(false));
}

#[test]
fn test_factory_request_spawn() {
    let req: FactoryRequest = serde_json::from_str(
        r#"{
            "action": "spawn_workers",
            "count": 3
        }"#,
    )
    .unwrap();
    assert_eq!(req.action, "spawn_workers");
    assert_eq!(req.count, Some(3));
    assert!(req.worker_names.is_none());
    assert!(req.target.is_none());
    assert!(req.message.is_none());
}

#[test]
fn spawn_workers_lane_survives_factory_and_coordination_decoding() {
    let factory: FactoryRequest =
        serde_json::from_str(r#"{"action":"spawn_workers","count":2,"lane":"standard"}"#).unwrap();
    assert_eq!(factory.lane.as_deref(), Some("standard"));

    let coordination: CoordinationRequest =
        serde_json::from_str(r#"{"action":"spawn_workers","count":2,"lane":"standard"}"#).unwrap();
    assert_eq!(
        coordination.to_factory_request().lane.as_deref(),
        Some("standard")
    );
}

#[test]
fn test_system_request_report_bug() {
    let req: SystemRequest = serde_json::from_str(
        r#"{
            "action": "report_cas_bug",
            "title": "Search fails silently",
            "description": "When search returns no results, no error is shown",
            "expected": "Should show 'no results' message",
            "actual": "Shows nothing"
        }"#,
    )
    .unwrap();
    assert_eq!(req.action, "report_cas_bug");
    assert_eq!(req.title, Some("Search fails silently".to_string()));
    assert_eq!(
        req.description,
        Some("When search returns no results, no error is shown".to_string())
    );
    assert_eq!(
        req.expected,
        Some("Should show 'no results' message".to_string())
    );
    assert_eq!(req.actual, Some("Shows nothing".to_string()));
}

#[test]
fn test_spec_request_default() {
    let req: SpecRequest = serde_json::from_str(r#"{"action": "list"}"#).unwrap();
    assert_eq!(req.action, "list");
    assert!(req.id.is_none());
    assert!(req.title.is_none());
    assert!(req.spec_type.is_none());
}

#[test]
fn test_spec_request_create() {
    let req: SpecRequest = serde_json::from_str(
        r#"{
            "action": "create",
            "title": "User Authentication Epic",
            "summary": "Add OAuth2 login flow",
            "spec_type": "epic",
            "goals": "Enable social login,Improve security",
            "acceptance_criteria": "Users can log in with Google,Users can log in with GitHub"
        }"#,
    )
    .unwrap();
    assert_eq!(req.action, "create");
    assert_eq!(req.title, Some("User Authentication Epic".to_string()));
    assert_eq!(req.summary, Some("Add OAuth2 login flow".to_string()));
    assert_eq!(req.spec_type, Some("epic".to_string()));
    assert_eq!(
        req.goals,
        Some("Enable social login,Improve security".to_string())
    );
    assert_eq!(
        req.acceptance_criteria,
        Some("Users can log in with Google,Users can log in with GitHub".to_string())
    );
}

#[test]
fn test_spec_request_supersede() {
    let req: SpecRequest = serde_json::from_str(
        r#"{
            "action": "supersede",
            "id": "spec-abc123",
            "supersedes_id": "spec-old456",
            "new_version": true
        }"#,
    )
    .unwrap();
    assert_eq!(req.action, "supersede");
    assert_eq!(req.id, Some("spec-abc123".to_string()));
    assert_eq!(req.supersedes_id, Some("spec-old456".to_string()));
    assert_eq!(req.new_version, Some(true));
}

// ===== String-coercion tests (Claude Code serializes numbers as strings) =====

#[test]
fn test_task_request_priority_as_string() {
    let req: TaskRequest = serde_json::from_str(
        r#"{
            "action": "create",
            "title": "Test",
            "priority": "1"
        }"#,
    )
    .unwrap();
    assert_eq!(req.priority, Some(1));
}

#[test]
fn cross_project_create_attempt_identity_is_additive_at_mcp_boundary() {
    let req: TaskRequest = serde_json::from_str(
        r#"{
            "action": "create",
            "title": "Foreign work",
            "project": "target-project",
            "proposal_attempt_id": "attempt-2026-08-13:one"
        }"#,
    )
    .unwrap();
    assert_eq!(req.project.as_deref(), Some("target-project"));
    assert_eq!(
        req.proposal_attempt_id.as_deref(),
        Some("attempt-2026-08-13:one")
    );
}

#[test]
fn test_task_request_priority_null() {
    let req: TaskRequest = serde_json::from_str(
        r#"{
            "action": "list",
            "priority": null
        }"#,
    )
    .unwrap();
    assert_eq!(req.priority, None);
}

#[test]
fn test_task_request_priority_absent() {
    let req: TaskRequest = serde_json::from_str(r#"{"action": "list"}"#).unwrap();
    assert_eq!(req.priority, None);
}

#[test]
fn test_factory_request_count_as_string() {
    let req: FactoryRequest = serde_json::from_str(
        r#"{
            "action": "spawn_workers",
            "count": "3"
        }"#,
    )
    .unwrap();
    assert_eq!(req.count, Some(3));
}

#[test]
fn test_coordination_request_count_as_string() {
    let req: CoordinationRequest = serde_json::from_str(
        r#"{
            "action": "spawn_workers",
            "count": "3",
            "isolate": true
        }"#,
    )
    .unwrap();
    assert_eq!(req.count, Some(3));
}

#[test]
fn test_coordination_request_count_as_int() {
    // Existing integer encoding must still work
    let req: CoordinationRequest = serde_json::from_str(
        r#"{
            "action": "shutdown_workers",
            "count": 0
        }"#,
    )
    .unwrap();
    assert_eq!(req.count, Some(0));
}

#[test]
fn test_coordination_request_count_null() {
    let req: CoordinationRequest =
        serde_json::from_str(r#"{"action": "worker_status", "count": null}"#).unwrap();
    assert_eq!(req.count, None);
}

// ===== option_i64 tests =====

#[test]
fn test_task_duration_secs_as_string() {
    let req: TaskRequest =
        serde_json::from_str(r#"{"action": "claim", "id": "t1", "duration_secs": "900"}"#).unwrap();
    assert_eq!(req.duration_secs, Some(900));
}

#[test]
fn test_task_duration_secs_as_int() {
    let req: TaskRequest =
        serde_json::from_str(r#"{"action": "claim", "id": "t1", "duration_secs": 600}"#).unwrap();
    assert_eq!(req.duration_secs, Some(600));
}

#[test]
fn test_task_duration_secs_null() {
    let req: TaskRequest =
        serde_json::from_str(r#"{"action": "claim", "duration_secs": null}"#).unwrap();
    assert_eq!(req.duration_secs, None);
}

#[test]
fn test_task_duration_secs_absent() {
    let req: TaskRequest = serde_json::from_str(r#"{"action": "claim"}"#).unwrap();
    assert_eq!(req.duration_secs, None);
}

#[test]
fn test_agent_stale_threshold_as_string() {
    let req: AgentRequest =
        serde_json::from_str(r#"{"action": "cleanup", "stale_threshold_secs": "3600"}"#).unwrap();
    assert_eq!(req.stale_threshold_secs, Some(3600));
}

#[test]
fn test_coordination_notification_id_as_string() {
    let req: CoordinationRequest =
        serde_json::from_str(r#"{"action": "queue_ack", "notification_id": "42"}"#).unwrap();
    assert_eq!(req.notification_id, Some(42));
}

#[test]
fn coordination_merge_request_type_survives_agent_mapping() {
    let req: CoordinationRequest = serde_json::from_str(
        r#"{"action":"message","target":"supervisor","task_id":"cas-89e1","merge_request":true,"summary":"ready to merge","message":"please merge"}"#,
    )
    .unwrap();

    assert_eq!(req.merge_request, Some(true));
    assert_eq!(req.to_agent_request("message").merge_request, Some(true));
}

/// cas-8725: the blocker flag must survive the same mapping. Dropped here it
/// would be silently ignored — the message would still be delivered, and would
/// still never wake the supervisor, which is exactly the failure this flag
/// exists to end.
#[test]
fn coordination_blocker_type_survives_agent_mapping() {
    let req: CoordinationRequest = serde_json::from_str(
        r#"{"action":"message","target":"supervisor","task_id":"cas-8725","blocker":true,"summary":"blocked on schema review","message":"the epic branch will not rebase"}"#,
    )
    .unwrap();

    assert_eq!(req.blocker, Some(true));
    assert_eq!(req.to_agent_request("message").blocker, Some(true));

    // Absent means absent: an ordinary message must not acquire the envelope.
    let plain: CoordinationRequest = serde_json::from_str(
        r#"{"action":"message","target":"supervisor","summary":"status","message":"still building"}"#,
    )
    .unwrap();
    assert_eq!(plain.blocker, None);
    assert_eq!(plain.to_agent_request("message").blocker, None);
}

#[test]
fn coordination_reply_reference_survives_agent_mapping() {
    let req: CoordinationRequest = serde_json::from_str(
        r#"{"action":"message","target":"worker-a","in_reply_to":7602,"summary":"acknowledged","message":"I read your escalation."}"#,
    )
    .unwrap();

    assert_eq!(req.in_reply_to, Some(7602));
    assert_eq!(req.to_agent_request("message").in_reply_to, Some(7602));
}

#[test]
fn test_factory_older_than_secs_as_string() {
    let req: FactoryRequest =
        serde_json::from_str(r#"{"action": "gc_cleanup", "older_than_secs": "7200"}"#).unwrap();
    assert_eq!(req.older_than_secs, Some(7200));
}

#[test]
fn test_coordination_maps_target_cache_dry_run_to_factory_request() {
    let req: CoordinationRequest =
        serde_json::from_str(r#"{"action":"gc_cleanup","force":true,"dry_run":false}"#).unwrap();
    let factory = req.to_factory_request();
    assert_eq!(factory.force, Some(true));
    assert_eq!(factory.dry_run, Some(false));
}

#[test]
fn spawn_workers_config_dir_survives_coordination_to_factory_mapping() {
    let req: CoordinationRequest = serde_json::from_str(
        r#"{"action":"spawn_workers","cli":"claude","config_dir":"~/.claude-alt"}"#,
    )
    .unwrap();

    let factory = req.to_factory_request();
    assert_eq!(factory.config_dir.as_deref(), Some("~/.claude-alt"));
}

#[test]
fn spawn_workers_per_worker_specs_survive_coordination_to_factory_mapping() {
    let req: CoordinationRequest = serde_json::from_str(
        r#"{"action":"spawn_workers","workers":"[{\"name\":\"research\",\"cli\":\"codex\",\"config_dir\":\"~/.codex-work\"}]"}"#,
    )
    .unwrap();

    let factory = req.to_factory_request();
    assert_eq!(
        factory.workers.as_deref(),
        Some(r#"[{"name":"research","cli":"codex","config_dir":"~/.codex-work"}]"#)
    );
}

#[test]
fn test_factory_remind_fields_as_string() {
    let req: FactoryRequest = serde_json::from_str(
        r#"{
            "action": "remind",
            "remind_delay_secs": "120",
            "remind_ttl_secs": "3600",
            "remind_id": "7"
        }"#,
    )
    .unwrap();
    assert_eq!(req.remind_delay_secs, Some(120));
    assert_eq!(req.remind_ttl_secs, Some(3600));
    assert_eq!(req.remind_id, Some(7));
    assert_eq!(
        req.cross_session, None,
        "reminders default to session-scoped"
    );
}

#[test]
fn cross_session_reminder_opt_in_survives_coordination_mapping() {
    let req: CoordinationRequest =
        serde_json::from_str(
            r#"{"action":"remind","task_id":"cas-issue-276","remind_delay_secs":120,"cross_session":true}"#,
        )
            .unwrap();
    assert_eq!(req.cross_session, Some(true));
    assert_eq!(req.to_factory_request().cross_session, Some(true));
    assert_eq!(
        req.to_factory_request().task_id.as_deref(),
        Some("cas-issue-276")
    );
}

// ===== option_u32 tests =====

#[test]
fn test_agent_max_iterations_as_string() {
    let req: AgentRequest =
        serde_json::from_str(r#"{"action": "loop_start", "max_iterations": "10"}"#).unwrap();
    assert_eq!(req.max_iterations, Some(10));
}

#[test]
fn test_agent_max_iterations_as_int() {
    let req: AgentRequest =
        serde_json::from_str(r#"{"action": "loop_start", "max_iterations": 5}"#).unwrap();
    assert_eq!(req.max_iterations, Some(5));
}

#[test]
fn test_agent_max_iterations_null() {
    let req: AgentRequest =
        serde_json::from_str(r#"{"action": "loop_start", "max_iterations": null}"#).unwrap();
    assert_eq!(req.max_iterations, None);
}

#[test]
fn test_agent_max_iterations_absent() {
    let req: AgentRequest = serde_json::from_str(r#"{"action": "loop_start"}"#).unwrap();
    assert_eq!(req.max_iterations, None);
}

#[test]
fn test_coordination_max_iterations_as_string() {
    let req: CoordinationRequest =
        serde_json::from_str(r#"{"action": "loop_start", "max_iterations": "20"}"#).unwrap();
    assert_eq!(req.max_iterations, Some(20));
}

// ===== option_usize tests =====

#[test]
fn test_memory_limit_as_string() {
    let req: MemoryRequest = serde_json::from_str(r#"{"action": "list", "limit": "50"}"#).unwrap();
    assert_eq!(req.limit, Some(50));
}

#[test]
fn test_memory_limit_as_int() {
    let req: MemoryRequest = serde_json::from_str(r#"{"action": "list", "limit": 25}"#).unwrap();
    assert_eq!(req.limit, Some(25));
}

#[test]
fn test_memory_limit_null() {
    let req: MemoryRequest = serde_json::from_str(r#"{"action": "list", "limit": null}"#).unwrap();
    assert_eq!(req.limit, None);
}

#[test]
fn test_memory_limit_absent() {
    let req: MemoryRequest = serde_json::from_str(r#"{"action": "list"}"#).unwrap();
    assert_eq!(req.limit, None);
}

#[test]
fn test_task_limit_as_string() {
    let req: TaskRequest = serde_json::from_str(r#"{"action": "list", "limit": "100"}"#).unwrap();
    assert_eq!(req.limit, Some(100));
}

#[test]
fn test_rule_limit_as_string() {
    let req: RuleRequest = serde_json::from_str(r#"{"action": "list", "limit": "10"}"#).unwrap();
    assert_eq!(req.limit, Some(10));
}

#[test]
fn test_skill_limit_as_string() {
    let req: SkillRequest = serde_json::from_str(r#"{"action": "list", "limit": "15"}"#).unwrap();
    assert_eq!(req.limit, Some(15));
}

#[test]
fn test_spec_limit_as_string() {
    let req: SpecRequest = serde_json::from_str(r#"{"action": "list", "limit": "20"}"#).unwrap();
    assert_eq!(req.limit, Some(20));
}

#[test]
fn test_search_max_tokens_as_string() {
    let req: SearchContextRequest =
        serde_json::from_str(r#"{"action": "context", "max_tokens": "4096"}"#).unwrap();
    assert_eq!(req.max_tokens, Some(4096));
}

#[test]
fn test_search_context_lines_as_string() {
    let req: SearchContextRequest = serde_json::from_str(
        r#"{"action": "grep", "pattern": "foo", "before_context": "3", "after_context": "5"}"#,
    )
    .unwrap();
    assert_eq!(req.before_context, Some(3));
    assert_eq!(req.after_context, Some(5));
}

#[test]
fn test_search_line_range_as_string() {
    let req: SearchContextRequest = serde_json::from_str(
        r#"{"action": "blame", "file_path": "src/main.rs", "line_start": "10", "line_end": "20"}"#,
    )
    .unwrap();
    assert_eq!(req.line_start, Some(10));
    assert_eq!(req.line_end, Some(20));
}

#[test]
fn test_search_limit_as_string() {
    let req: SearchContextRequest =
        serde_json::from_str(r#"{"action": "search", "query": "test", "limit": "30"}"#).unwrap();
    assert_eq!(req.limit, Some(30));
    assert_eq!(req.provenance_version, None);
    assert_eq!(req.query_id, None);
    assert_eq!(req.outcome, None);
}

#[test]
fn test_retrieval_feedback_fields_are_additive_and_typed() {
    let req: SearchContextRequest = serde_json::from_str(
        r#"{
            "action": "retrieval_feedback",
            "query_id": "qry-123",
            "result_id": "cas-abcd",
            "outcome": "corrected",
            "actor_id": "worker-1",
            "session_id": "session-1",
            "correction_ref": "cas-ef01",
            "provenance_version": "1"
        }"#,
    )
    .unwrap();
    assert_eq!(req.provenance_version, Some(1));
    assert_eq!(req.query_id.as_deref(), Some("qry-123"));
    assert_eq!(req.result_id.as_deref(), Some("cas-abcd"));
    assert_eq!(req.outcome.as_deref(), Some("corrected"));
    assert_eq!(req.actor_id.as_deref(), Some("worker-1"));
    assert_eq!(req.correction_ref.as_deref(), Some("cas-ef01"));
}

#[test]
fn test_team_limit_as_string() {
    let req: TeamRequest = serde_json::from_str(r#"{"action": "list", "limit": "5"}"#).unwrap();
    assert_eq!(req.limit, Some(5));
}

#[test]
fn test_pattern_limit_as_string() {
    let req: PatternRequest = serde_json::from_str(r#"{"action": "list", "limit": "8"}"#).unwrap();
    assert_eq!(req.limit, Some(8));
}

#[test]
fn test_coordination_limit_as_string() {
    let req: CoordinationRequest =
        serde_json::from_str(r#"{"action": "agent_list", "limit": "50"}"#).unwrap();
    assert_eq!(req.limit, Some(50));
}

// ===== option_u64 tests =====

#[test]
fn test_verification_duration_ms_as_string() {
    let req: VerificationRequest =
        serde_json::from_str(r#"{"action": "add", "task_id": "t1", "duration_ms": "1500"}"#)
            .unwrap();
    assert_eq!(req.duration_ms, Some(1500));
}

#[test]
fn test_verification_duration_ms_as_int() {
    let req: VerificationRequest =
        serde_json::from_str(r#"{"action": "add", "task_id": "t1", "duration_ms": 2000}"#).unwrap();
    assert_eq!(req.duration_ms, Some(2000));
}

#[test]
fn test_verification_duration_ms_null() {
    let req: VerificationRequest =
        serde_json::from_str(r#"{"action": "add", "task_id": "t1", "duration_ms": null}"#).unwrap();
    assert_eq!(req.duration_ms, None);
}

#[test]
fn test_verification_duration_ms_absent() {
    let req: VerificationRequest =
        serde_json::from_str(r#"{"action": "add", "task_id": "t1"}"#).unwrap();
    assert_eq!(req.duration_ms, None);
    assert_eq!(req.verifier_capability, None);
    assert_eq!(req.dispatch_id, None);
    let serialized = serde_json::to_value(&req).unwrap();
    assert!(
        serialized.get("verifier_capability").is_none(),
        "legacy request serialization must omit the additive capability field"
    );
    assert!(
        serialized.get("dispatch_id").is_none(),
        "legacy request serialization must omit the additive dispatch field"
    );
}

#[test]
fn test_verification_limit_as_string() {
    let req: VerificationRequest =
        serde_json::from_str(r#"{"action": "list", "task_id": "t1", "limit": "10"}"#).unwrap();
    assert_eq!(req.limit, Some(10));
}

// ===== ExecuteRequest max_length =====

#[test]
fn test_execute_max_length_as_string() {
    let req: ExecuteRequest =
        serde_json::from_str(r#"{"code": "return 1;", "max_length": "5000"}"#).unwrap();
    assert_eq!(req.max_length, Some(5000));
}

#[test]
fn test_execute_max_length_as_int() {
    let req: ExecuteRequest =
        serde_json::from_str(r#"{"code": "return 1;", "max_length": 10000}"#).unwrap();
    assert_eq!(req.max_length, Some(10000));
}

// ===== Empty string coercion to None =====

#[test]
fn test_empty_string_coerces_to_none() {
    let req: TaskRequest =
        serde_json::from_str(r#"{"action": "list", "priority": "", "limit": ""}"#).unwrap();
    assert_eq!(req.priority, None);
    assert_eq!(req.limit, None);
}

// ===== Coordination request fields from factory/agent =====

#[test]
fn test_coordination_all_numeric_fields_as_string() {
    let req: CoordinationRequest = serde_json::from_str(
        r#"{
            "action": "remind",
            "count": "2",
            "max_iterations": "10",
            "stale_threshold_secs": "300",
            "notification_id": "99",
            "older_than_secs": "7200",
            "remind_delay_secs": "60",
            "remind_id": "5",
            "remind_ttl_secs": "1800",
            "limit": "25"
        }"#,
    )
    .unwrap();
    assert_eq!(req.count, Some(2));
    assert_eq!(req.max_iterations, Some(10));
    assert_eq!(req.stale_threshold_secs, Some(300));
    assert_eq!(req.notification_id, Some(99));
    assert_eq!(req.older_than_secs, Some(7200));
    assert_eq!(req.remind_delay_secs, Some(60));
    assert_eq!(req.remind_id, Some(5));
    assert_eq!(req.remind_ttl_secs, Some(1800));
    assert_eq!(req.limit, Some(25));
}

// ============================================================================
// cas-ca63: priority alias + flexible bool deserializers
// ============================================================================

#[test]
fn test_priority_accepts_named_aliases() {
    for (alias, expected) in &[
        ("critical", 0u8),
        ("CRITICAL", 0),
        ("high", 1),
        ("medium", 2),
        ("normal", 2),
        ("low", 3),
        ("backlog", 4),
        ("p0", 0),
        ("p1", 1),
        ("p4", 4),
    ] {
        let json = format!(r#"{{"action":"create","title":"x","priority":"{alias}"}}"#);
        let req: TaskRequest = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("priority alias '{alias}' should parse: {e}"));
        assert_eq!(
            req.priority,
            Some(*expected),
            "priority alias '{alias}' should map to {expected}"
        );
    }
}

#[test]
fn test_priority_accepts_numeric_string() {
    let req: TaskRequest =
        serde_json::from_str(r#"{"action":"create","title":"x","priority":"1"}"#).unwrap();
    assert_eq!(req.priority, Some(1));
}

#[test]
fn test_priority_accepts_numeric() {
    let req: TaskRequest =
        serde_json::from_str(r#"{"action":"create","title":"x","priority":3}"#).unwrap();
    assert_eq!(req.priority, Some(3));
}

#[test]
fn test_priority_rejects_unknown_alias_with_helpful_error() {
    let err = serde_json::from_str::<TaskRequest>(
        r#"{"action":"create","title":"x","priority":"urgent"}"#,
    )
    .expect_err("unknown alias must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("critical") && msg.contains("high") && msg.contains("backlog"),
        "error must list valid aliases so the caller knows what to use: {msg}"
    );
}

#[test]
fn test_priority_rejects_out_of_range_numeric() {
    let err =
        serde_json::from_str::<TaskRequest>(r#"{"action":"create","title":"x","priority":9}"#)
            .expect_err("priority 9 is out of 0-4 range");
    let msg = err.to_string();
    assert!(
        msg.contains("critical") || msg.contains("0-4"),
        "error must hint at valid range: {msg}"
    );
}

#[test]
fn test_with_deps_accepts_string_bool() {
    // cas-ca63 Issue 2: boolean as string should be coerced.
    let req: TaskRequest =
        serde_json::from_str(r#"{"action":"show","id":"cas-1","with_deps":"true"}"#).unwrap();
    assert_eq!(req.with_deps, Some(true));

    let req: TaskRequest =
        serde_json::from_str(r#"{"action":"show","id":"cas-1","with_deps":"false"}"#).unwrap();
    assert_eq!(req.with_deps, Some(false));

    let req: TaskRequest =
        serde_json::from_str(r#"{"action":"show","id":"cas-1","with_deps":1}"#).unwrap();
    assert_eq!(req.with_deps, Some(true));

    let req: TaskRequest =
        serde_json::from_str(r#"{"action":"show","id":"cas-1","with_deps":true}"#).unwrap();
    assert_eq!(req.with_deps, Some(true));
}

#[test]
fn test_with_deps_rejects_garbage_string_with_helpful_error() {
    let err = serde_json::from_str::<TaskRequest>(
        r#"{"action":"show","id":"cas-1","with_deps":"maybe"}"#,
    )
    .expect_err("garbage bool string must be rejected");
    let msg = err.to_string();
    assert!(
        msg.contains("boolean") && msg.contains("true"),
        "error must show valid values: {msg}"
    );
}
