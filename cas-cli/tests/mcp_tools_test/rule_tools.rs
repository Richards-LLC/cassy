use crate::support::*;
use cas::mcp::tools::*;
use cas::store::open_rule_store;
use cas::types::RuleStatus;
use rmcp::handler::server::wrapper::Parameters;

#[tokio::test]
async fn test_rule_create() {
    let (_temp, service) = setup_cas();

    let req = RuleCreateRequest {
        scope: "project".to_string(),
        content: "Always use snake_case for function names".to_string(),
        paths: Some("**/*.rs".to_string()),
        tags: Some("style,naming".to_string()),
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
async fn test_rule_harmful_demotes_proven_rule_and_removes_injection() {
    let (temp, service) = setup_cas();

    let result = service
        .cas_rule_create(Parameters(RuleCreateRequest {
            scope: "project".to_string(),
            content: "Demotion test rule".to_string(),
            paths: None,
            tags: None,
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
async fn test_rule_delete() {
    let (_temp, service) = setup_cas();

    // Create rule
    let req = RuleCreateRequest {
        scope: "project".to_string(),
        content: "Delete test rule".to_string(),
        paths: None,
        tags: None,
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
