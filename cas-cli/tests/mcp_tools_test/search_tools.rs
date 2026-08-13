use crate::support::*;
use cas::mcp::tools::service::SearchContextRequest;
use cas::mcp::tools::*;
use rmcp::handler::server::wrapper::Parameters;
use rmcp::model::ErrorCode;

fn seed_projection_documents(cas_root: &std::path::Path) {
    let index_dir = cas_root.join("index/tantivy");
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

    let metrics_req: SearchContextRequest =
        serde_json::from_value(serde_json::json!({"action": "retrieval_metrics"})).unwrap();
    let metrics = service
        .search(Parameters(metrics_req))
        .await
        .expect("offline metrics should succeed");
    let metrics_json: serde_json::Value =
        serde_json::from_str(&extract_text(metrics)).expect("metrics response should be JSON");
    assert_eq!(metrics_json["groups"][0]["document_type"], "task");
    assert_eq!(
        metrics_json["groups"][0]["ranking_policy"],
        "current-default-v1"
    );
    assert_eq!(metrics_json["groups"][0]["helpful"], 1);
    assert_eq!(metrics_json["groups"][0]["usefulness_rate"], 1.0);

    let db = std::fs::read(temp.path().join(".cas/cas.db")).unwrap();
    let raw = String::from_utf8_lossy(&db);
    assert!(!raw.contains("  Retrieval   PROVENANCE integration  "));
    assert!(!raw.contains("private-integration-actor"));
    assert!(!raw.contains("private-integration-session"));
}
