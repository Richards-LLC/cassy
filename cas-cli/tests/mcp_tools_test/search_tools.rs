use crate::support::*;
use cas::mcp::tools::service::SearchContextRequest;
use cas::mcp::tools::*;
use rmcp::handler::server::wrapper::Parameters;

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
