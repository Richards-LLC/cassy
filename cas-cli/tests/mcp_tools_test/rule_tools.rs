use crate::support::*;
use cas::mcp::tools::*;
use cas::store::open_rule_store;
use cas::types::{Rule, RuleStatus};
use cas_store::{RetrievalHitIdentity, RetrievalOutcome, RetrievalStore, SqliteRetrievalStore};
use rmcp::handler::server::wrapper::Parameters;

#[tokio::test]
async fn test_rule_create() {
    let (_temp, service) = setup_cas();

    let req = RuleCreateRequest {
        scope: "project".to_string(),
        content: "Always use snake_case for function names".to_string(),
        paths: Some("**/*.rs".to_string()),
        tags: Some("style,naming".to_string()),
        source_ids: None,
        auto_approve_tools: None,
        auto_approve_paths: None,
    };

    let result = service
        .cas_rule_create(Parameters(req))
        .await
        .expect("rule_create should succeed");

    let text = extract_text(result);
    assert!(text.contains("Created rule") || text.contains("rule"));
}

#[tokio::test]
async fn test_rule_show() {
    let (_temp, service) = setup_cas();

    // Create rule
    let req = RuleCreateRequest {
        scope: "project".to_string(),
        content: "Test rule for show".to_string(),
        paths: None,
        tags: None,
        source_ids: Some("learning-1, learning-2".to_string()),
        auto_approve_tools: None,
        auto_approve_paths: None,
    };

    let result = service
        .cas_rule_create(Parameters(req))
        .await
        .expect("rule_create should succeed");

    let text = extract_text(result);
    let id = text
        .split('[')
        .nth(1)
        .and_then(|s| s.split(']').next())
        .or_else(|| {
            text.split("rule-")
                .nth(1)
                .and_then(|s| s.split(|c: char| !c.is_alphanumeric()).next())
        })
        .expect("should have rule ID");

    let rule_id = if id.starts_with("rule-") {
        id.to_string()
    } else {
        format!("rule-{id}")
    };

    // Show rule
    let show_req = IdRequest {
        id: rule_id.clone(),
    };
    let result = service
        .cas_rule_show(Parameters(show_req))
        .await
        .expect("rule_show should succeed");

    let text = extract_text(result);
    assert!(text.contains("Test rule for show") || text.contains(&rule_id));
    assert!(text.contains("learning-1, learning-2"));
}

#[tokio::test]
async fn test_rule_impact_metrics_are_visible_in_list_and_show() {
    let (_temp, service) = setup_cas();
    let store = open_rule_store(&_temp.path().join(".cas")).unwrap();
    let mut rule = Rule::new(
        "rule-impact".to_string(),
        "Rule with impact metrics".to_string(),
    );
    rule.status = RuleStatus::Proven;
    rule.surface_count = 7;
    rule.helpful_count = 3;
    rule.harmful_count = 1;
    store.add(&rule).unwrap();

    let show = service
        .cas_rule_show(Parameters(IdRequest {
            id: rule.id.clone(),
        }))
        .await
        .unwrap();
    let show_text = extract_text(show);
    assert!(show_text.contains("Impact: surfaced 7"));
    assert!(show_text.contains("+3 helpful, -1 harmful"));

    let list = service.cas_rules_list().await.unwrap();
    let list_text = extract_text(list);
    assert!(list_text.contains("Impact: surfaced 7"));
    assert!(list_text.contains("+3 helpful, -1 harmful"));

    let all = service
        .cas_rule_list_all(Parameters(LimitRequest {
            scope: "all".to_string(),
            limit: Some(10),
            sort: None,
            sort_order: None,
            team_id: None,
        }))
        .await
        .unwrap();
    let all_text = extract_text(all);
    assert!(all_text.contains("surfaced: 7"));
    assert!(all_text.contains("feedback: +3 -1"));
}

#[tokio::test]
async fn test_rule_list() {
    let (_temp, service) = setup_cas();

    // Create rules
    for i in 0..3 {
        let req = RuleCreateRequest {
            scope: "project".to_string(),
            content: format!("List rule {i}"),
            paths: None,
            tags: None,
            source_ids: None,
            auto_approve_tools: None,
            auto_approve_paths: None,
        };
        service
            .cas_rule_create(Parameters(req))
            .await
            .expect("rule_create should succeed");
    }

    // List all rules
    let list_req = LimitRequest {
        scope: "all".to_string(),
        limit: Some(10),
        sort: None,
        sort_order: None,
        team_id: None,
    };
    let result = service
        .cas_rule_list_all(Parameters(list_req))
        .await
        .expect("rule_list_all should succeed");

    let text = extract_text(result);
    assert!(text.contains("List rule") || text.contains("Rules") || text.contains("rule"));
}

#[tokio::test]
async fn test_rule_update() {
    let (_temp, service) = setup_cas();

    // Create rule
    let req = RuleCreateRequest {
        scope: "project".to_string(),
        content: "Original rule content".to_string(),
        paths: None,
        tags: None,
        source_ids: None,
        auto_approve_tools: None,
        auto_approve_paths: None,
    };

    let result = service
        .cas_rule_create(Parameters(req))
        .await
        .expect("rule_create should succeed");

    let text = extract_text(result);
    let id = extract_rule_id(&text).expect("should have rule ID");

    // Update rule
    let update_req = RuleUpdateRequest {
        id: id.clone(),
        content: Some("Updated rule content".to_string()),
        paths: Some("**/*.ts".to_string()),
        tags: None,
        auto_approve_tools: None,
        auto_approve_paths: None,
    };

    let result = service
        .cas_rule_update(Parameters(update_req))
        .await
        .expect("rule_update should succeed");

    let text = extract_text(result);
    assert!(text.contains("Updated") || text.contains("updated"));
}

#[tokio::test]
async fn test_rule_helpful_and_harmful() {
    let (_temp, service) = setup_cas();

    // Create rule
    let req = RuleCreateRequest {
        scope: "project".to_string(),
        content: "Feedback test rule".to_string(),
        paths: None,
        tags: None,
        source_ids: None,
        auto_approve_tools: None,
        auto_approve_paths: None,
    };

    let result = service
        .cas_rule_create(Parameters(req))
        .await
        .expect("rule_create should succeed");

    let text = extract_text(result);
    let id = if text.contains("rule-") {
        text.split("rule-")
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_alphanumeric()).next())
            .map(|s| format!("rule-{s}"))
            .expect("should have rule ID")
    } else {
        text.split('[')
            .nth(1)
            .and_then(|s| s.split(']').next())
            .expect("should have rule ID")
            .to_string()
    };

    // Mark helpful
    let helpful_req = IdRequest { id: id.clone() };
    let result = service
        .cas_rule_helpful(Parameters(helpful_req))
        .await
        .expect("rule_helpful should succeed");

    let text = extract_text(result);
    assert!(text.contains("helpful") || text.contains("Promoted") || text.contains("Proven"));

    // Mark harmful
    let harmful_req = IdRequest { id: id.clone() };
    let result = service
        .cas_rule_harmful(Parameters(harmful_req))
        .await
        .expect("rule_harmful should succeed");

    let text = extract_text(result);
    assert!(text.contains("harmful"));
}

#[tokio::test]
async fn test_rule_helpful_requires_evidence_threshold() {
    let (_temp, service) = setup_cas();

    let result = service
        .cas_rule_create(Parameters(RuleCreateRequest {
            scope: "project".to_string(),
            content: "Threshold test rule".to_string(),
            paths: None,
            tags: None,
            source_ids: None,
            auto_approve_tools: None,
            auto_approve_paths: None,
        }))
        .await
        .unwrap();
    let id = extract_rule_id(&extract_text(result)).expect("rule ID");

    service
        .cas_rule_helpful(Parameters(IdRequest { id: id.clone() }))
        .await
        .unwrap();

    let rule = open_rule_store(&_temp.path().join(".cas"))
        .unwrap()
        .get(&id)
        .unwrap();
    assert_eq!(rule.helpful_count, 1);
    assert_eq!(rule.status, RuleStatus::Draft);
}

#[tokio::test]
async fn test_rule_helpful_promotes_at_configured_threshold() {
    let (_temp, service) = setup_cas();
    std::fs::write(
        _temp.path().join(".cas/config.toml"),
        "[sync]\npromotion_threshold = 3\n",
    )
    .unwrap();

    let result = service
        .cas_rule_create(Parameters(RuleCreateRequest {
            scope: "project".to_string(),
            content: "Configured threshold rule".to_string(),
            paths: None,
            tags: None,
            source_ids: None,
            auto_approve_tools: None,
            auto_approve_paths: None,
        }))
        .await
        .unwrap();
    let id = extract_rule_id(&extract_text(result)).expect("rule ID");

    for expected_status in [RuleStatus::Draft, RuleStatus::Draft, RuleStatus::Proven] {
        service
            .cas_rule_helpful(Parameters(IdRequest { id: id.clone() }))
            .await
            .unwrap();
        let rule = open_rule_store(&_temp.path().join(".cas"))
            .unwrap()
            .get(&id)
            .unwrap();
        assert_eq!(rule.status, expected_status);
    }
}

#[tokio::test]
async fn test_rule_helpful_accepts_configured_retrieval_evidence() {
    let (temp, service) = setup_cas();
    std::fs::write(
        temp.path().join(".cas/config.toml"),
        "[sync]\npromotion_threshold = 2\npromotion_evidence = [\"retrieval\"]\n",
    )
    .unwrap();

    let result = service
        .cas_rule_create(Parameters(RuleCreateRequest {
            scope: "project".to_string(),
            content: "Retrieval evidence rule".to_string(),
            paths: None,
            tags: None,
            source_ids: None,
            auto_approve_tools: None,
            auto_approve_paths: None,
        }))
        .await
        .unwrap();
    let id = extract_rule_id(&extract_text(result)).expect("rule ID");

    let retrieval = SqliteRetrievalStore::open(&temp.path().join(".cas")).unwrap();
    for (query_id, outcome_id, session_id, actor_id) in [
        (
            "query-rule-promotion-1",
            "outcome-rule-promotion-1",
            "session-1",
            "actor-1",
        ),
        (
            "query-rule-promotion-2",
            "outcome-rule-promotion-2",
            "session-2",
            "actor-2",
        ),
    ] {
        retrieval
            .record_query(
                query_id,
                "rule evidence",
                "rule_validation",
                "test-policy",
                Some(session_id),
                &[RetrievalHitIdentity {
                    result_id: id.clone(),
                    document_type: "rule".to_string(),
                    rank: 0,
                }],
            )
            .unwrap();
        retrieval
            .record_outcome(
                outcome_id,
                query_id,
                &id,
                RetrievalOutcome::Helpful,
                actor_id,
                session_id,
                None,
            )
            .unwrap();
    }

    service
        .cas_rule_helpful(Parameters(IdRequest { id: id.clone() }))
        .await
        .unwrap();
    let rule = open_rule_store(&temp.path().join(".cas"))
        .unwrap()
        .get(&id)
        .unwrap();
    assert_eq!(rule.status, RuleStatus::Proven);
}

#[tokio::test]
async fn test_rule_harmful_demotes_proven_rule_and_removes_injection() {
    let (temp, service) = setup_cas();

    let result = service
        .cas_rule_create(Parameters(RuleCreateRequest {
            scope: "project".to_string(),
            content: "Demotion test rule".to_string(),
            paths: None,
            tags: None,
            source_ids: None,
            auto_approve_tools: None,
            auto_approve_paths: None,
        }))
        .await
        .unwrap();
    let id = extract_rule_id(&extract_text(result)).expect("rule ID");

    for _ in 0..2 {
        service
            .cas_rule_helpful(Parameters(IdRequest { id: id.clone() }))
            .await
            .unwrap();
    }
    let injected = temp
        .path()
        .join(".claude/rules/cas")
        .join(format!("{id}.md"));
    assert!(injected.is_file(), "proven rule should be injected");

    service
        .cas_rule_harmful(Parameters(IdRequest { id: id.clone() }))
        .await
        .unwrap();
    let rule = open_rule_store(&temp.path().join(".cas"))
        .unwrap()
        .get(&id)
        .unwrap();
    assert_eq!(rule.status, RuleStatus::Stale);
    assert!(!injected.exists(), "stale rule must stop being injected");
}

#[tokio::test]
async fn test_corrected_retrieval_evidence_demotes_on_sync() {
    let (temp, service) = setup_cas();

    let result = service
        .cas_rule_create(Parameters(RuleCreateRequest {
            scope: "project".to_string(),
            content: "Corrected retrieval rule".to_string(),
            paths: None,
            tags: None,
            source_ids: None,
            auto_approve_tools: None,
            auto_approve_paths: None,
        }))
        .await
        .unwrap();
    let id = extract_rule_id(&extract_text(result)).expect("rule ID");
    for _ in 0..2 {
        service
            .cas_rule_helpful(Parameters(IdRequest { id: id.clone() }))
            .await
            .unwrap();
    }

    let injected = temp
        .path()
        .join(".claude/rules/cas")
        .join(format!("{id}.md"));
    assert!(injected.is_file(), "proven rule should be injected");

    let retrieval = SqliteRetrievalStore::open(&temp.path().join(".cas")).unwrap();
    retrieval
        .record_query(
            "query-rule-correction",
            "rule correction",
            "rule_validation",
            "test-policy",
            Some("session-correction"),
            &[RetrievalHitIdentity {
                result_id: id.clone(),
                document_type: "rule".to_string(),
                rank: 0,
            }],
        )
        .unwrap();
    retrieval
        .record_outcome(
            "outcome-rule-correction",
            "query-rule-correction",
            &id,
            RetrievalOutcome::Corrected,
            "actor-correction",
            "session-correction",
            Some("correction-1"),
        )
        .unwrap();

    service.cas_rule_sync().await.unwrap();
    let rule = open_rule_store(&temp.path().join(".cas"))
        .unwrap()
        .get(&id)
        .unwrap();
    assert_eq!(rule.status, RuleStatus::Stale);
    assert!(!injected.exists(), "corrected rule must stop being injected");
}

#[tokio::test]
async fn test_existing_proven_rule_is_grandfathered() {
    let (temp, service) = setup_cas();
    std::fs::write(
        temp.path().join(".cas/config.toml"),
        "[sync]\npromotion_threshold = 5\n",
    )
    .unwrap();

    let mut rule = Rule::new(
        "rule-grandfathered".to_string(),
        "Existing proven rule".to_string(),
    );
    rule.status = RuleStatus::Proven;
    open_rule_store(&temp.path().join(".cas"))
        .unwrap()
        .add(&rule)
        .unwrap();

    service.cas_rule_sync().await.unwrap();
    let stored = open_rule_store(&temp.path().join(".cas"))
        .unwrap()
        .get("rule-grandfathered")
        .unwrap();
    assert_eq!(stored.status, RuleStatus::Proven);
}

#[tokio::test]
async fn test_rule_delete() {
    let (_temp, service) = setup_cas();

    // Create rule
    let req = RuleCreateRequest {
        scope: "project".to_string(),
        content: "Delete test rule".to_string(),
        paths: None,
        tags: None,
        source_ids: None,
        auto_approve_tools: None,
        auto_approve_paths: None,
    };

    let result = service
        .cas_rule_create(Parameters(req))
        .await
        .expect("rule_create should succeed");

    let text = extract_text(result);
    let id = if text.contains("rule-") {
        text.split("rule-")
            .nth(1)
            .and_then(|s| s.split(|c: char| !c.is_alphanumeric()).next())
            .map(|s| format!("rule-{s}"))
            .expect("should have rule ID")
    } else {
        text.split('[')
            .nth(1)
            .and_then(|s| s.split(']').next())
            .expect("should have rule ID")
            .to_string()
    };

    // Delete rule
    let delete_req = IdRequest { id: id.clone() };
    let result = service
        .cas_rule_delete(Parameters(delete_req))
        .await
        .expect("rule_delete should succeed");

    let text = extract_text(result);
    assert!(text.contains("Deleted"));
}
