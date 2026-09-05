use crate::support::*;
use cas::hooks::HookInput;
use cas::mcp::tools::service::SearchContextRequest;
use cas::mcp::tools::*;
use cas_store::{
    DEFAULT_RETRIEVAL_POLICY, RETRIEVAL_ATTRIBUTION_AUTOMATIC, RETRIEVAL_ATTRIBUTION_JUDGE,
    RetrievalHitIdentity, RetrievalOutcome, RetrievalStore, SqliteRetrievalStore, SqliteStore,
    Store,
};
use cas_types::{Agent, Entry};
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::ErrorCode;

fn seed_projection_documents(cas_root: &std::path::Path) {
    let index_dir = cas::hybrid_search::tantivy_index_dir(cas_root);
    drop(cas::hybrid_search::SearchIndex::open(&index_dir).unwrap());

    let index = tantivy::Index::open_in_dir(index_dir).unwrap();
    let schema = index.schema();
    let id = schema.get_field("id").unwrap();
    let content = schema.get_field("content").unwrap();
    let tags = schema.get_field("tags").unwrap();
    let kind = schema.get_field("type").unwrap();
    let title = schema.get_field("title").unwrap();
    let doc_type = schema.get_field("doc_type").unwrap();
    let mut writer = index.writer(50_000_000).unwrap();

    for (document_type, marker) in [
        ("entry", "projectionentrye626"),
        ("task", "projectiontaske626"),
        ("rule", "projectionrulee626"),
        ("skill", "projectionskille626"),
        ("code_symbol", "projectionsymbole626"),
        ("code_file", "projectionfilee626"),
        ("spec", "projectionspece626"),
    ] {
        let mut document = tantivy::TantivyDocument::new();
        document.add_text(id, format!("{document_type}-e626"));
        document.add_text(content, marker);
        document.add_text(tags, "");
        document.add_text(kind, "test");
        document.add_text(title, marker);
        document.add_text(doc_type, document_type);
        writer.add_document(document).unwrap();
    }

    // A same-query decoy proves that `doc_type=spec` is an actual filter,
    // rather than merely allowing the requested projection to rank first.
    let mut decoy = tantivy::TantivyDocument::new();
    decoy.add_text(id, "entry-spec-decoy-e626");
    decoy.add_text(content, "projectionspece626");
    decoy.add_text(tags, "");
    decoy.add_text(kind, "test");
    decoy.add_text(title, "projectionspece626");
    decoy.add_text(doc_type, "entry");
    writer.add_document(decoy).unwrap();
    writer.commit().unwrap();
}

async fn provenance_search(
    service: &CasService,
    query: &str,
    doc_type: Option<&str>,
    version: usize,
) -> Result<serde_json::Value, rmcp::ErrorData> {
    let request: SearchContextRequest = serde_json::from_value(serde_json::json!({
        "action": "search",
        "query": query,
        "doc_type": doc_type,
        "limit": 10,
        "provenance_version": version
    }))
    .unwrap();
    service
        .search(Parameters(request))
        .await
        .map(|result| serde_json::from_str(&extract_text(result)).unwrap())
}

#[tokio::test]
async fn test_search_empty() {
    let (_temp, service) = setup_cas();

    let req = SearchRequest {
        scope: "all".to_string(),
        query: "nonexistent content".to_string(),
        doc_type: None,
        limit: 10,
        tags: None,
    };

    let result = service
        .cas_search(Parameters(req))
        .await
        .expect("search should succeed");

    let text = extract_text(result);
    // Compatibility contract: omitting provenance_version preserves the
    // existing response byte-for-byte.
    assert_eq!(text, "No results found");
}

#[tokio::test]
async fn test_search_with_content() {
    let (_temp, service) = setup_cas();

    // Create searchable content
    let req = RememberRequest {
        scope: "project".to_string(),
        content: "Searchable unique memory content for testing search functionality".to_string(),
        entry_type: "learning".to_string(),
        tags: Some("search,test".to_string()),
        title: None,
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };

    service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    // Search for it (may need index to be built first)
    let search_req = SearchRequest {
        scope: "all".to_string(),
        query: "searchable unique memory".to_string(),
        doc_type: Some("entry".to_string()),
        limit: 10,
        tags: None,
    };

    let result = service
        .cas_search(Parameters(search_req))
        .await
        .expect("search should succeed");

    // Search result depends on whether index was built
    let text = extract_text(result);
    assert!(!text.is_empty());
}

#[tokio::test]
async fn skill_impact_reports_surface_rows_and_session_outcomes() {
    use cas_store::{RuleStore, SkillStore, SqliteRuleStore, SqliteSkillStore, SqliteStore};
    use cas_types::{Rule, RuleStatus, Session, SessionOutcome, Skill, SkillStatus};

    let (temp, core) = setup_cas();
    let cas_dir = temp.path().join(".cas");

    let rule_store = SqliteRuleStore::open(&cas_dir).unwrap();
    let mut rule = Rule::new("rule-impact".to_string(), "Impact rule".to_string());
    rule.status = RuleStatus::Proven;
    rule.helpful_count = 2;
    rule.harmful_count = 1;
    rule_store.add(&rule).unwrap();

    let skill_store = SqliteSkillStore::open(&cas_dir).unwrap();
    let mut skill = Skill::new("skill-impact".to_string(), "Impact skill".to_string());
    skill.status = SkillStatus::Enabled;
    skill.usage_count = 4;
    skill_store.add(&skill).unwrap();

    let session_store = SqliteStore::open(&cas_dir).unwrap();
    let session = Session::new("impact-session".to_string(), "/repo".to_string(), None);
    session_store.start_session(&session).unwrap();
    session_store
        .update_session_outcome("impact-session", SessionOutcome::TasksCompleted)
        .unwrap();

    let surface_store = cas_store::SqliteSurfacedArtifactStore::open(&cas_dir).unwrap();
    surface_store
        .record_batch(
            "impact-session",
            &[
                cas_store::SurfacedArtifact {
                    artifact_id: "rule-impact".to_string(),
                    artifact_type: "rule".to_string(),
                    preview: Some("Impact rule".to_string()),
                },
                cas_store::SurfacedArtifact {
                    artifact_id: "skill-impact".to_string(),
                    artifact_type: "skill".to_string(),
                    preview: Some("Impact skill".to_string()),
                },
            ],
        )
        .unwrap();

    let service = CasService::new(core, None);
    let request: SearchContextRequest = serde_json::from_value(serde_json::json!({
        "action": "skill_impact",
        "limit": 10
    }))
    .unwrap();
    let response = service
        .search(Parameters(request))
        .await
        .expect("skill impact report should succeed");
    let report: serde_json::Value =
        serde_json::from_str(&extract_text(response)).expect("impact response should be JSON");

    assert_eq!(report["version"], 1);
    let artifacts = report["artifacts"].as_array().unwrap();
    let rule_impact = artifacts
        .iter()
        .find(|artifact| artifact["artifact_id"] == "rule-impact")
        .unwrap();
    assert_eq!(rule_impact["surfaced_count"], 1);
    assert_eq!(rule_impact["outcome_counts"]["tasks_completed"], 1);
    assert_eq!(rule_impact["helpful_count"], 2);
    assert_eq!(rule_impact["harmful_count"], 1);
    let skill_impact = artifacts
        .iter()
        .find(|artifact| artifact["artifact_id"] == "skill-impact")
        .unwrap();
    assert_eq!(skill_impact["usage_count"], 4);
    assert_eq!(skill_impact["outcome_counts"]["tasks_completed"], 1);
}

#[tokio::test]
async fn cas_57e5_colon_bearing_free_text_queries_return_results() {
    let (_temp, service) = setup_cas();
    service
        .cas_remember(Parameters(RememberRequest {
            scope: "project".to_string(),
            content: "test colon handling 12:49 https://example.invalid/path foo::bar".to_string(),
            entry_type: "learning".to_string(),
            tags: None,
            title: None,
            importance: 0.5,
            valid_from: None,
            valid_until: None,
            team_id: None,
            bypass_overlap: None,
            mode: None,
            expected_updated_at: None,
            personal: None,
        }))
        .await
        .expect("remember should succeed");

    for query in [
        "test colon handling 12:49",
        "https://example.invalid/path",
        "foo::bar",
    ] {
        let result = service
            .cas_search(Parameters(SearchRequest {
                scope: "all".to_string(),
                query: query.to_string(),
                doc_type: Some("entry".to_string()),
                limit: 10,
                tags: None,
            }))
            .await
            .unwrap_or_else(|error| panic!("{query:?} must not fail parsing: {error}"));
        assert!(
            extract_text(result).contains("[Entry]"),
            "expected literal query {query:?} to return the remembered entry"
        );
    }
}

#[tokio::test]
async fn artifact_fixture_content_is_searchable_after_backfill() {
    let (temp, service) = setup_cas();
    let artifacts_root = temp.path().join("durable-artifacts");
    let task_id = "cas-artifact-fixture";
    let artifact = artifacts_root.join(task_id).join("SEND-LOG.md");
    std::fs::create_dir_all(artifact.parent().unwrap()).unwrap();
    std::fs::write(
        &artifact,
        "SES MessageId: artifact-fixture-unique-message-id-292",
    )
    .unwrap();
    std::fs::write(
        temp.path().join(".cas/config.toml"),
        format!(
            "[factory]\nartifacts_root = {:?}\n",
            artifacts_root.display().to_string()
        ),
    )
    .unwrap();

    service
        .cas_reindex(Parameters(ReindexRequest {
            bm25: true,
            embeddings: false,
            missing_only: false,
        }))
        .await
        .expect("artifact backfill should succeed");

    let result = service
        .cas_search(Parameters(SearchRequest {
            scope: "project".to_string(),
            query: "artifact-fixture-unique-message-id-292".to_string(),
            doc_type: Some("artifact".to_string()),
            limit: 10,
            tags: None,
        }))
        .await
        .expect("artifact search should succeed");

    let output = extract_text(result);
    assert!(output.contains("[Artifact]"));
    assert!(output.contains(task_id));
    assert!(output.contains(artifact.to_str().unwrap()));
}

#[tokio::test]
async fn cas_4caa_expired_memory_is_excluded_from_search_recall() {
    let (_temp, service) = setup_cas();
    let request = RememberRequest {
        scope: "project".to_string(),
        content: "expired search recall marker cas4caa".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: None,
        importance: 0.5,
        valid_from: None,
        valid_until: Some((chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339()),
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };
    service
        .cas_remember(Parameters(request))
        .await
        .expect("remember should succeed");

    let result = service
        .cas_search(Parameters(SearchRequest {
            scope: "all".to_string(),
            query: "expired search recall marker cas4caa".to_string(),
            doc_type: Some("entry".to_string()),
            limit: 10,
            tags: None,
        }))
        .await
        .expect("search should succeed");
    assert_eq!(extract_text(result), "No results found");
}

#[tokio::test]
async fn test_search_filter_by_type() {
    let (_temp, service) = setup_cas();

    // Create content
    let req = RememberRequest {
        scope: "project".to_string(),
        content: "Filter test memory".to_string(),
        entry_type: "learning".to_string(),
        tags: None,
        title: None,
        importance: 0.5,
        valid_from: None,
        valid_until: None,
        team_id: None,
        bypass_overlap: None,
        mode: None,
        expected_updated_at: None,
        personal: None,
    };

    service
        .cas_remember(Parameters(req))
        .await
        .expect("remember should succeed");

    let task_req = TaskCreateRequest {
        depth: None,
        title: "Filter test task".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    service
        .cas_task_create(Parameters(task_req))
        .await
        .expect("task_create should succeed");

    // Search only tasks
    let search_req = SearchRequest {
        scope: "all".to_string(),
        query: "filter test".to_string(),
        doc_type: Some("task".to_string()),
        limit: 10,
        tags: None,
    };

    let result = service
        .cas_search(Parameters(search_req))
        .await
        .expect("search should succeed");

    let text = extract_text(result);
    // Should only return tasks if any match
    if !text.contains("No results") {
        // If we got results, they should be task-related
        assert!(!text.contains("entry") || text.contains("task"));
    }
}

#[tokio::test]
async fn provenance_v1_rejects_unsupported_versions_at_the_public_boundary() {
    let (_temp, core) = setup_cas();
    let service = CasService::new(core, None);

    let error = provenance_search(&service, "anything", None, 2)
        .await
        .unwrap_err();
    assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
    assert_eq!(
        error.message,
        "unsupported provenance_version 2; expected 1"
    );
}

#[tokio::test]
async fn provenance_v1_empty_results_classify_each_query_family() {
    let (_temp, core) = setup_cas();
    let service = CasService::new(core, None);

    for (query, doc_type, expected_family) in [
        ("plainkeywordmissinge626", None, "keyword"),
        ("what is missing e626?", None, "question"),
        ("\"missing:e626\"", None, "filtered"),
        ("cas-dead", None, "id_lookup"),
        ("missingcodee626", Some("code_symbol"), "code"),
    ] {
        let envelope = provenance_search(&service, query, doc_type, 1)
            .await
            .unwrap();
        assert_eq!(envelope["version"], 1, "{query}");
        assert_eq!(envelope["schema"], "cas.retrieval.provenance.v1", "{query}");
        assert_eq!(envelope["query_family"], expected_family, "{query}");
        assert_eq!(envelope["hits"], serde_json::json!([]), "{query}");
    }
}

#[tokio::test]
async fn provenance_v1_projects_every_unified_document_type() {
    let (temp, core) = setup_cas();
    seed_projection_documents(&temp.path().join(".cas"));
    let service = CasService::new(core, None);

    for (document_type, marker, preview_prefix, origin, scope) in [
        ("entry", "projectionentrye626", "[Entry]", None, "unknown"),
        ("task", "projectiontaske626", "[Task]", None, "project"),
        ("rule", "projectionrulee626", "[Rule]", None, "unknown"),
        ("skill", "projectionskille626", "[Skill]", None, "unknown"),
        (
            "code_symbol",
            "projectionsymbole626",
            "[Code]",
            Some("code_index"),
            "project",
        ),
        (
            "code_file",
            "projectionfilee626",
            "[File]",
            Some("code_index"),
            "project",
        ),
        ("spec", "projectionspece626", "[Spec]", None, "project"),
    ] {
        let envelope = provenance_search(&service, marker, Some(document_type), 1)
            .await
            .unwrap();
        let hits = envelope["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1, "{document_type}: {envelope}");
        let hit = &hits[0];
        assert_eq!(hit["document_type"], document_type);
        assert!(
            hit["preview"].as_str().unwrap().starts_with(preview_prefix),
            "{document_type}: {hit}"
        );
        assert_eq!(
            hit["provenance"]["source"]["origin"],
            origin.map_or(serde_json::Value::Null, serde_json::Value::from)
        );
        assert_eq!(hit["provenance"]["source"]["index"], "tantivy_unified_v1");
        assert_eq!(hit["provenance"]["scope"], scope);
    }
}

#[tokio::test]
async fn test_versioned_provenance_feedback_and_offline_metrics_flow() {
    let (temp, core) = setup_cas();

    let cas_dir = temp.path().join(".cas");
    let entry_store = SqliteStore::open(&cas_dir).unwrap();
    for id in ["metrics-session-memory-a", "metrics-session-memory-b"] {
        entry_store
            .add(&Entry::new(
                id.to_string(),
                format!("Helpful SessionStart metric memory {id}"),
            ))
            .unwrap();
    }
    let session_input = HookInput {
        session_id: "metrics-context-session".to_string(),
        cwd: temp.path().to_string_lossy().to_string(),
        hook_event_name: "SessionStart".to_string(),
        ..Default::default()
    };
    cas::hooks::build_context_with_token_budget(&session_input, 2, &cas_dir, None)
        .expect("SessionStart context should build");

    let task_req = TaskCreateRequest {
        depth: None,
        title: "Retrieval provenance integration marker".to_string(),
        description: None,
        priority: 2,
        task_type: "task".to_string(),
        labels: None,
        notes: None,
        blocked_by: None,
        design: None,
        acceptance_criteria: None,
        external_ref: None,
        assignee: None,
        demo_statement: None,
        execution_note: None,
        epic: None,
    };
    let created = core
        .cas_task_create(Parameters(task_req))
        .await
        .expect("task_create should succeed");
    let task_id = extract_task_id(&extract_text(created))
        .expect("created task should expose its ID")
        .to_string();
    core.cas_task_update(Parameters(TaskUpdateRequest {
        blocked_by: None,
        depth: None,
        id: task_id.clone(),
        title: None,
        notes: None,
        priority: None,
        labels: None,
        description: None,
        design: None,
        acceptance_criteria: None,
        demo_statement: None,
        execution_note: None,
        external_ref: None,
        assignee: None,
        status: Some("blocked".to_string()),
        epic: None,
        origin_project: None,
        epic_verification_owner: None,
    }))
    .await
    .expect("task update should make conflict metadata observable");

    let legacy_query = "  Retrieval   PROVENANCE integration  ";
    let legacy = core
        .cas_search(Parameters(SearchRequest {
            scope: "all".to_string(),
            query: legacy_query.to_string(),
            doc_type: Some("task".to_string()),
            limit: 10,
            tags: None,
        }))
        .await
        .expect("legacy search should succeed");
    assert_eq!(
        extract_text(legacy),
        format!(
            "Search results for \"{legacy_query}\":\n\n\
             1. [Task] P2 Blocked Retrieval provenance integration marker (score: 1.73)\n   \
             ID: {task_id}\n\n\
             Found 1 results"
        ),
        "omitting provenance_version must preserve the non-empty legacy response byte-for-byte"
    );

    let service = CasService::new(core, None);
    let provenance_req: SearchContextRequest = serde_json::from_value(serde_json::json!({
        "action": "search",
        "query": legacy_query,
        "doc_type": "task",
        "limit": 10,
        "provenance_version": 1,
        "session_id": "private-integration-session"
    }))
    .unwrap();
    let response = service
        .search(Parameters(provenance_req))
        .await
        .expect("provenance search should succeed");
    let envelope: serde_json::Value =
        serde_json::from_str(&extract_text(response)).expect("structured response should be JSON");
    assert_eq!(envelope["version"], 1);
    assert_eq!(envelope["schema"], "cas.retrieval.provenance.v1");
    assert_eq!(envelope["ranking_policy"], "current-default-v1");
    let query_id = envelope["query_id"].as_str().unwrap().to_string();
    let hit = &envelope["hits"][0];
    assert_eq!(hit["document_type"], "task");
    assert_eq!(hit["id"], task_id);
    assert_eq!(hit["provenance"]["source"]["index"], "tantivy_unified_v1");
    assert!(hit["provenance"]["scores"]["final_score"].is_number());
    assert!(hit["provenance"]["freshness"]["stale"].is_boolean());
    assert_eq!(hit["provenance"]["conflict"], true);
    assert_eq!(hit["provenance"]["signals"], serde_json::json!(["blocked"]));
    let result_id = hit["id"].as_str().unwrap().to_string();

    let feedback_req: SearchContextRequest = serde_json::from_value(serde_json::json!({
        "action": "retrieval_feedback",
        "query_id": query_id,
        "result_id": result_id,
        "outcome": "helpful",
        "actor_id": "private-integration-actor",
        "session_id": "private-integration-session"
    }))
    .unwrap();
    let feedback = service
        .search(Parameters(feedback_req))
        .await
        .expect("explicit feedback should persist");
    let feedback_json: serde_json::Value =
        serde_json::from_str(&extract_text(feedback)).expect("feedback response should be JSON");
    assert_eq!(feedback_json["outcome"], "helpful");
    assert_eq!(feedback_json["attribution"], "explicit");

    let unresolved_req: SearchContextRequest = serde_json::from_value(serde_json::json!({
        "action": "retrieval_feedback",
        "query_id": feedback_json["query_id"],
        "result_id": feedback_json["result_id"],
        "outcome": "unresolved",
        "actor_id": "private-integration-actor",
        "session_id": "private-integration-session"
    }))
    .unwrap();
    service
        .search(Parameters(unresolved_req))
        .await
        .expect("unresolved telemetry should persist independently of resolved feedback");

    let metrics_req: SearchContextRequest =
        serde_json::from_value(serde_json::json!({"action": "retrieval_metrics"})).unwrap();
    let metrics = service
        .search(Parameters(metrics_req))
        .await
        .expect("offline metrics should succeed");
    let metrics_json: serde_json::Value =
        serde_json::from_str(&extract_text(metrics)).expect("metrics response should be JSON");
    assert!(metrics_json["rolling_injected_precision"].is_null());
    assert_eq!(metrics_json["injected_precision_numerator"], 0);
    assert_eq!(metrics_json["injected_precision_denominator"], 0);
    assert_eq!(metrics_json["injected_precision_window_days"], 30);
    let groups = metrics_json["groups"]
        .as_array()
        .expect("metrics groups should be an array");
    let task_group = groups
        .iter()
        .find(|group| group["document_type"] == "task")
        .expect("task provenance group should be present");
    assert_eq!(task_group["ranking_policy"], "current-default-v1");
    assert_eq!(task_group["total"], 2);
    assert_eq!(task_group["helpful"], 1);
    assert_eq!(task_group["resolved"], 1);
    assert_eq!(task_group["unresolved"], 1);
    assert_eq!(task_group["results"], 1);
    assert_eq!(task_group["denominator"], "resolved");
    assert_eq!(task_group["coverage_rate"], 1.0);
    assert_eq!(task_group["usefulness_rate"], 1.0);

    let context_group = groups
        .iter()
        .find(|group| group["query_family"] == "context_session_start")
        .expect("SessionStart memory telemetry group should be present");
    assert_eq!(context_group["document_type"], "entry");
    assert_eq!(context_group["results"], 2);
    assert_eq!(context_group["denominator"], "resolved");
    assert_eq!(context_group["coverage_rate"], 0.0);

    let db = std::fs::read(temp.path().join(".cas/cas.db")).unwrap();
    let raw = String::from_utf8_lossy(&db);
    assert!(!raw.contains("  Retrieval   PROVENANCE integration  "));
    assert!(!raw.contains("private-integration-actor"));
    assert!(!raw.contains("private-integration-session"));
}

#[tokio::test]
async fn retrieval_metrics_filters_by_session_and_rejects_unsupported_filters() {
    let (temp, core) = setup_cas();
    let cas_dir = temp.path().join(".cas");
    let store = SqliteRetrievalStore::open(&cas_dir).expect("retrieval store should open");
    let agent_store = cas::store::open_agent_store(&cas_dir).expect("agent store should open");
    for (id, name) in [
        ("session-a", "worker-a"),
        ("session-b", "worker-b"),
        ("session-empty", "worker-empty"),
    ] {
        let mut agent = Agent::new(id.to_string(), name.to_string());
        agent.factory_session = Some("factory-fixture".to_string());
        agent_store
            .register(&agent)
            .expect("fixture agent should register");
    }
    let hits = [RetrievalHitIdentity {
        result_id: "entry-session-filter".to_string(),
        document_type: "entry".to_string(),
        rank: 0,
    }];
    for (query_id, session_id, query_family) in [
        ("query-session-a", "session-a", "ambient_session_start"),
        ("query-session-b", "session-b", "context_session_start"),
        (
            "query-session-historical",
            "session-historical",
            "ambient_transition",
        ),
    ] {
        store
            .record_query(
                query_id,
                "session-filter query",
                query_family,
                DEFAULT_RETRIEVAL_POLICY,
                Some(session_id),
                &hits,
            )
            .expect("retrieval query should persist");
    }
    store
        .record_outcome_with_attribution(
            "opened-session-a",
            "query-session-a",
            "entry-session-filter",
            RetrievalOutcome::Used,
            "automatic-hook",
            "session-a",
            None,
            RETRIEVAL_ATTRIBUTION_AUTOMATIC,
        )
        .expect("automatic body pull should persist");
    store
        .record_outcome(
            "used-session-historical",
            "query-session-historical",
            "entry-session-filter",
            RetrievalOutcome::Used,
            "explicit-agent",
            "session-historical",
            None,
        )
        .expect("explicit use should persist");
    for (event_id, query_id, session_id, outcome) in [
        (
            "judge-session-a",
            "query-session-a",
            "session-a",
            RetrievalOutcome::Ignored,
        ),
        (
            "judge-session-b",
            "query-session-b",
            "session-b",
            RetrievalOutcome::Helpful,
        ),
    ] {
        store
            .record_outcome_with_attribution(
                event_id,
                query_id,
                "entry-session-filter",
                outcome,
                "fixture-judge",
                session_id,
                None,
                RETRIEVAL_ATTRIBUTION_JUDGE,
            )
            .expect("judge label should persist");
    }
    for (event_id, attribution) in [
        ("judge-used-session-b", RETRIEVAL_ATTRIBUTION_JUDGE),
        ("custom-used-session-b", "future-signal"),
    ] {
        store
            .record_outcome_with_attribution(
                event_id,
                "query-session-b",
                "entry-session-filter",
                RetrievalOutcome::Used,
                "non-caller",
                "session-b",
                None,
                attribution,
            )
            .expect("non-explicit Used control should persist");
    }

    let service = CasService::new(core, None);
    let metrics = |session_id: Option<&str>| {
        serde_json::json!({
            "action": "retrieval_metrics",
            "session_id": session_id,
        })
    };
    let response = service
        .search(Parameters(
            serde_json::from_value(metrics(None)).expect("unfiltered request should deserialize"),
        ))
        .await
        .expect("unfiltered metrics should succeed");
    let all: serde_json::Value =
        serde_json::from_str(&extract_text(response)).expect("metrics should be JSON");
    assert_eq!(all["groups"].as_array().unwrap().len(), 3);
    assert_eq!(all["injected_precision_numerator"], 1);
    assert_eq!(all["injected_precision_denominator"], 2);
    assert_eq!(all["retrieval_funnel"]["retrieved"], 3);
    assert_eq!(all["retrieval_funnel"]["injected"], 3);
    assert_eq!(all["retrieval_funnel"]["opened"], 1);
    assert_eq!(all["retrieval_funnel"]["used"], 1);
    assert_eq!(all["retrieval_funnel"]["judged_helpful"], 1);
    assert_eq!(all["session_scope"]["status"], "available");
    assert_eq!(all["session_scope"]["identity_kind"], "all_sessions");

    let response = service
        .search(Parameters(
            serde_json::from_value(metrics(Some("session-a")))
                .expect("session-filtered request should deserialize"),
        ))
        .await
        .expect("session-filtered metrics should succeed");
    let filtered: serde_json::Value =
        serde_json::from_str(&extract_text(response)).expect("filtered metrics should be JSON");
    assert_eq!(filtered["groups"][0]["results"], 1);
    assert_eq!(filtered["injected_precision_numerator"], 0);
    assert_eq!(filtered["injected_precision_denominator"], 1);
    assert_eq!(filtered["retrieval_funnel"]["retrieved"], 1);
    assert_eq!(filtered["retrieval_funnel"]["injected"], 1);
    assert_eq!(filtered["retrieval_funnel"]["opened"], 1);
    assert_eq!(filtered["retrieval_funnel"]["used"], 0);
    assert_eq!(filtered["retrieval_funnel"]["judged_helpful"], 0);
    assert_eq!(filtered["session_scope"]["status"], "available");
    assert_eq!(filtered["session_scope"]["identity_kind"], "agent_session");
    assert_eq!(filtered["judge_measurement"]["status"], "available");

    let response = service
        .search(Parameters(
            serde_json::from_value(metrics(Some("session-historical")))
                .expect("historical session request should deserialize"),
        ))
        .await
        .expect("historical session metrics should remain valid from stored evidence");
    let historical: serde_json::Value =
        serde_json::from_str(&extract_text(response)).expect("historical metrics should be JSON");
    assert_eq!(historical["groups"][0]["results"], 1);
    assert_eq!(historical["retrieval_funnel"]["opened"], 0);
    assert_eq!(historical["retrieval_funnel"]["used"], 1);
    assert_eq!(historical["retrieval_funnel"]["judged_helpful"], 0);
    assert_eq!(historical["session_scope"]["status"], "available");
    assert_eq!(
        historical["session_scope"]["identity_kind"],
        "stored_retrieval_session"
    );

    let response = service
        .search(Parameters(
            serde_json::from_value(metrics(Some("session-empty")))
                .expect("valid empty session request should deserialize"),
        ))
        .await
        .expect("valid empty session metrics should succeed");
    let empty: serde_json::Value =
        serde_json::from_str(&extract_text(response)).expect("empty metrics should be JSON");
    assert_eq!(empty["groups"], serde_json::json!([]));
    assert_eq!(empty["session_scope"]["status"], "valid_empty");
    assert_eq!(empty["session_scope"]["identity_kind"], "agent_session");

    let response = service
        .search(Parameters(
            serde_json::from_value(metrics(Some("factory-fixture")))
                .expect("factory session request should deserialize"),
        ))
        .await
        .expect("factory session metrics should explain the identity mismatch");
    let factory: serde_json::Value =
        serde_json::from_str(&extract_text(response)).expect("factory metrics should be JSON");
    assert_eq!(factory["groups"], serde_json::json!([]));
    assert_eq!(factory["session_scope"]["status"], "invalid_identity_kind");
    assert_eq!(factory["session_scope"]["identity_kind"], "factory_session");
    assert_eq!(factory["session_scope"]["matching_agent_sessions"], 3);

    let response = service
        .search(Parameters(
            serde_json::from_value(metrics(Some("worker-a")))
                .expect("agent name request should deserialize"),
        ))
        .await
        .expect("agent name metrics should explain the identity mismatch");
    let named: serde_json::Value =
        serde_json::from_str(&extract_text(response)).expect("named metrics should be JSON");
    assert_eq!(named["groups"], serde_json::json!([]));
    assert_eq!(named["session_scope"]["status"], "invalid_identity_kind");
    assert_eq!(named["session_scope"]["identity_kind"], "agent_name");
    assert_eq!(named["session_scope"]["canonical_session_id"], "session-a");

    let response = service
        .search(Parameters(
            serde_json::from_value(metrics(Some("session-missing")))
                .expect("different session request should deserialize"),
        ))
        .await
        .expect("different session metrics should succeed");
    let missing: serde_json::Value =
        serde_json::from_str(&extract_text(response)).expect("missing metrics should be JSON");
    assert_eq!(missing["groups"], serde_json::json!([]));
    assert_eq!(missing["session_scope"]["status"], "unknown");
    assert_eq!(missing["session_scope"]["identity_kind"], "unknown");
    assert_eq!(missing["judge_measurement"]["status"], "unavailable");
    assert_eq!(missing["judge_measurement"]["reason"], "judge_unconfigured");
    assert_eq!(
        missing["judge_measurement"]["scheduled_judge_configured"],
        false
    );

    let unsupported: SearchContextRequest = serde_json::from_value(serde_json::json!({
        "action": "retrieval_metrics",
        "doc_type": "entry",
    }))
    .expect("unsupported filter request should deserialize");
    let error = service
        .search(Parameters(unsupported))
        .await
        .expect_err("unsupported metrics filters must fail explicitly");
    assert_eq!(error.code, ErrorCode::INVALID_PARAMS);
    assert!(
        error.message.contains("doc_type"),
        "unexpected error: {error:?}"
    );
}
