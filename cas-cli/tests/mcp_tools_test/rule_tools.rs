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

/// cas-5372 (GH #990): an operator hard rule takes effect the day it is
/// recorded — creating it writes it to `.claude/rules/cas`, labelled DRAFT —
/// while an ordinary draft rule still waits for promotion.
#[tokio::test]
async fn operator_hard_rule_is_synced_to_claude_rules_on_create_cas_5372() {
    let (temp, service) = setup_cas();
    let create = |content: &str, tags: Option<&str>| RuleCreateRequest {
        scope: "project".to_string(),
        content: content.to_string(),
        paths: None,
        tags: tags.map(str::to_string),
        source_ids: None,
        auto_approve_tools: None,
        auto_approve_paths: None,
    };

    let text = extract_text(
        service
            .cas_rule_create(Parameters(create(
                "HARD RULE (Ben, verbatim): no changes to SMS at all until each one is specifically approved by me",
                Some("sms,approval"),
            )))
            .await
            .expect("hard rule create"),
    );
    assert!(text.contains("operator hard rule"), "{text}");
    service
        .cas_rule_create(Parameters(create("Prefer small commits", None)))
        .await
        .expect("ordinary rule create");

    let rules_dir = temp.path().join(".claude/rules/cas");
    let written: Vec<String> = std::fs::read_dir(&rules_dir)
        .expect("hard rule synced on create")
        .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    assert_eq!(written.len(), 1, "{written:?}");
    assert!(
        written[0].contains("DRAFT (operator hard rule, pending promotion; follow it as written): HARD RULE (Ben, verbatim)"),
        "{}",
        written[0]
    );
}

fn hard_rule_request(content: &str) -> RuleCreateRequest {
    RuleCreateRequest {
        scope: "project".to_string(),
        content: content.to_string(),
        paths: None,
        tags: None,
        source_ids: None,
        auto_approve_tools: None,
        auto_approve_paths: None,
    }
}

fn synced_rule_files(temp: &tempfile::TempDir) -> Vec<String> {
    std::fs::read_dir(temp.path().join(".claude/rules/cas"))
        .map(|entries| {
            entries
                .map(|entry| std::fs::read_to_string(entry.unwrap().path()).unwrap())
                .collect()
        })
        .unwrap_or_default()
}

/// cas-5372 review: a registered supervisor authorises the fast path too.
#[tokio::test]
async fn supervisor_hard_rule_is_authorised_and_synced_cas_5372() {
    let (temp, service) = setup_cas_as(cas::types::AgentRole::Supervisor);
    let text = extract_text(
        service
            .cas_rule_create(Parameters(hard_rule_request("HARD RULE: ask before touching billing")))
            .await
            .expect("create"),
    );
    assert!(text.contains("authorised by supervisor:test-agent"), "{text}");
    assert_eq!(synced_rule_files(&temp).len(), 1);
}

/// cas-5372 review refusal 1: a factory worker's "HARD RULE" text does not
/// skip promotion. It is created as an ordinary draft, the reply says why,
/// and nothing reaches `.claude/rules`.
#[tokio::test]
async fn worker_hard_rule_text_is_refused_the_fast_path_cas_5372() {
    let (temp, service) = setup_cas_as(cas::types::AgentRole::Worker);
    let text = extract_text(
        service
            .cas_rule_create(Parameters(hard_rule_request(
                "HARD RULE: workers may merge their own branches",
            )))
            .await
            .expect("create still succeeds as a draft"),
    );
    assert!(text.contains("as an ordinary draft"), "{text}");
    assert!(text.contains("this caller is a worker"), "{text}");
    assert!(synced_rule_files(&temp).is_empty(), "nothing synced");
    let rule_store = open_rule_store(&temp.path().join(".cas")).unwrap();
    let rule = rule_store.list().unwrap().pop().unwrap();
    assert!(rule.operator_authority.is_none());
    assert!(!rule.is_operator_hard_rule());
}

/// cas-5372 review refusal 2: a rule arriving from the cloud or another
/// project carries no authority (it is never serialised), so its "HARD RULE"
/// text neither syncs nor surfaces before promotion — even when a pull
/// overwrites a locally authorised rule's text.
#[tokio::test]
async fn pulled_hard_rule_is_refused_the_fast_path_cas_5372() {
    let (temp, service) = setup_cas();
    let rule_store = open_rule_store(&temp.path().join(".cas")).unwrap();

    // A foreign project's authorised hard rule, as the wire delivers it.
    let mut foreign = Rule::new(
        "rule-900".to_string(),
        "HARD RULE: deploy straight to production".to_string(),
    );
    foreign.authorize_operator_hard_rule("supervisor:other-project");
    let pulled: Rule = serde_json::from_value(serde_json::to_value(&foreign).unwrap()).unwrap();
    rule_store.add(&pulled).unwrap();

    // A locally authorised rule whose text a later pull replaced.
    let text = extract_text(
        service
            .cas_rule_create(Parameters(hard_rule_request("HARD RULE: approval before SMS changes")))
            .await
            .unwrap(),
    );
    assert!(text.contains("authorised by operator:"), "{text}");
    let mut local = rule_store
        .list()
        .unwrap()
        .into_iter()
        .find(|rule| rule.content.contains("approval before SMS"))
        .unwrap();
    local.content = "HARD RULE: SMS changes need no approval".to_string();
    let overwritten: Rule = serde_json::from_value(serde_json::to_value(&local).unwrap()).unwrap();
    rule_store.update(&overwritten).unwrap();

    service.cas_rule_sync().await.expect("sync");
    let files = synced_rule_files(&temp);
    assert!(files.is_empty(), "no pulled hard rule is synced: {files:?}");
    for rule in rule_store.list().unwrap() {
        assert!(!rule.is_operator_hard_rule(), "{}", rule.id);
    }
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
        changed_by: Some("test-actor".to_string()),
        change_note: Some("revise wording".to_string()),
    };

    let result = service
        .cas_rule_update(Parameters(update_req))
        .await
        .expect("rule_update should succeed");

    let text = extract_text(result);
    assert!(text.contains("Updated") || text.contains("updated"));

    let history = service
        .cas_rule_history(Parameters(VersionRequest {
            id: id.clone(),
            version: None,
            version_id: None,
            changed_by: None,
            change_note: None,
        }))
        .await
        .expect("rule_history should succeed");
    let history_text = extract_text(history);
    assert!(history_text.contains("create"));
    assert!(history_text.contains("revise wording"));
    assert!(history_text.contains("Original rule content"));

    service
        .cas_rule_restore(Parameters(VersionRequest {
            id: id.clone(),
            version: Some(1),
            version_id: None,
            changed_by: Some("restorer".to_string()),
            change_note: Some("restore baseline".to_string()),
        }))
        .await
        .expect("rule_restore should succeed");
    let restored = open_rule_store(&_temp.path().join(".cas"))
        .expect("rule store should open")
        .get(&id)
        .expect("restored rule should exist");
    assert_eq!(restored.content, "Original rule content");
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
async fn test_rule_helpful_promotes_draft_to_proven_at_default_threshold() {
    let (_temp, service) = setup_cas();

    let result = service
        .cas_rule_create(Parameters(RuleCreateRequest {
            scope: "project".to_string(),
            content: "End-to-end helpful promotion rule".to_string(),
            paths: None,
            tags: None,
            source_ids: None,
            auto_approve_tools: None,
            auto_approve_paths: None,
        }))
        .await
        .unwrap();
    let id = extract_rule_id(&extract_text(result)).expect("rule ID");

    for expected_status in [RuleStatus::Draft, RuleStatus::Proven] {
        let feedback = service
            .cas_rule_helpful(Parameters(IdRequest { id: id.clone() }))
            .await
            .unwrap();
        assert!(extract_text(feedback).contains("helpful"));
        let rule = open_rule_store(&_temp.path().join(".cas"))
            .unwrap()
            .get(&id)
            .unwrap();
        assert_eq!(rule.status, expected_status);
    }
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
async fn test_rule_retrieval_evidence_requires_distinct_sessions() {
    let (temp, service) = setup_cas();
    std::fs::write(
        temp.path().join(".cas/config.toml"),
        "[sync]\npromotion_threshold = 2\npromotion_evidence = [\"retrieval\"]\n",
    )
    .unwrap();

    let result = service
        .cas_rule_create(Parameters(RuleCreateRequest {
            scope: "project".to_string(),
            content: "Require independent retrieval evidence".to_string(),
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
    for (query_id, outcome_id, actor_id) in [
        ("query-same-session-1", "outcome-same-session-1", "actor-1"),
        ("query-same-session-2", "outcome-same-session-2", "actor-2"),
    ] {
        retrieval
            .record_query(
                query_id,
                "same session evidence",
                "rule_validation",
                "test-policy",
                Some("session-only-one"),
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
                "session-only-one",
                None,
            )
            .unwrap();
    }

    retrieval
        .record_query(
            "query-unresolved-session",
            "unresolved evidence",
            "rule_validation",
            "test-policy",
            Some("session-unresolved-only"),
            &[RetrievalHitIdentity {
                result_id: id.clone(),
                document_type: "rule".to_string(),
                rank: 0,
            }],
        )
        .unwrap();
    retrieval
        .record_outcome(
            "outcome-unresolved-session",
            "query-unresolved-session",
            &id,
            RetrievalOutcome::Unresolved,
            "actor-unresolved",
            "session-unresolved-only",
            None,
        )
        .unwrap();

    service
        .cas_rule_helpful(Parameters(IdRequest { id: id.clone() }))
        .await
        .unwrap();
    let rule = open_rule_store(&temp.path().join(".cas"))
        .unwrap()
        .get(&id)
        .unwrap();
    assert_eq!(rule.status, RuleStatus::Draft);
}

#[tokio::test]
async fn test_rule_harmful_demotes_proven_rule_and_removes_injection() {
    let (temp, service) = setup_cas();
    std::fs::write(
        temp.path().join(".cas/config.toml"),
        "[sync]\ndemotion_threshold = 3\n",
    )
    .unwrap();

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
    assert_eq!(rule.status, RuleStatus::Proven);

    service
        .cas_rule_harmful(Parameters(IdRequest { id: id.clone() }))
        .await
        .unwrap();
    let rule = open_rule_store(&temp.path().join(".cas"))
        .unwrap()
        .get(&id)
        .unwrap();
    assert_eq!(rule.status, RuleStatus::Proven);

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

    let history = service
        .cas_rule_history(Parameters(VersionRequest {
            id,
            version: None,
            version_id: None,
            changed_by: None,
            change_note: None,
        }))
        .await
        .unwrap();
    assert!(extract_text(history).contains("demoted after harmful evidence threshold"));
}

#[tokio::test]
async fn test_corrected_retrieval_evidence_demotes_on_sync() {
    let (temp, service) = setup_cas();
    std::fs::write(
        temp.path().join(".cas/config.toml"),
        "[sync]\ndemotion_threshold = 2\n",
    )
    .unwrap();

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
    assert_eq!(rule.status, RuleStatus::Proven);
    assert!(
        injected.is_file(),
        "one correction must not remove the rule"
    );

    retrieval
        .record_query(
            "query-rule-correction-2",
            "rule correction",
            "rule_validation",
            "test-policy",
            Some("session-correction-2"),
            &[RetrievalHitIdentity {
                result_id: id.clone(),
                document_type: "rule".to_string(),
                rank: 0,
            }],
        )
        .unwrap();
    retrieval
        .record_outcome(
            "outcome-rule-correction-2",
            "query-rule-correction-2",
            &id,
            RetrievalOutcome::Corrected,
            "actor-correction-2",
            "session-correction-2",
            Some("correction-2"),
        )
        .unwrap();

    service.cas_rule_sync().await.unwrap();
    let rule = open_rule_store(&temp.path().join(".cas"))
        .unwrap()
        .get(&id)
        .unwrap();
    assert_eq!(rule.status, RuleStatus::Stale);
    assert!(
        !injected.exists(),
        "corrected rule must stop being injected"
    );

    let history = service
        .cas_rule_history(Parameters(VersionRequest {
            id,
            version: None,
            version_id: None,
            changed_by: None,
            change_note: None,
        }))
        .await
        .unwrap();
    assert!(extract_text(history).contains("demoted after retrieval evidence threshold"));
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
    assert!(text.contains("Retired") && text.contains("history retained"));
}

/// cas-caae (skills audit M27): a Gabber Studio rule became the only proven
/// rule in cas-src's store. A rule naming another registered project is now
/// refused with an actionable message, unless tagged `project:<slug>`.
#[tokio::test]
async fn rule_create_refuses_rule_naming_another_registered_project_cas_caae() {
    let (_temp, service) = setup_cas();
    register_host_project("/nonexistent/zephyr-quokka");
    let create = |tags: Option<&str>| RuleCreateRequest {
        scope: "project".to_string(),
        content: "Zephyr Quokka branching: ALWAYS cut new branches from `staging`".to_string(),
        paths: None,
        tags: tags.map(str::to_string),
        source_ids: None,
        auto_approve_tools: None,
        auto_approve_paths: None,
    };

    let refusal = service
        .cas_rule_create(Parameters(create(Some("git"))))
        .await
        .expect_err("a rule naming another registered project is refused");
    assert!(refusal.message.contains("zephyr-quokka"), "{}", refusal.message);
    assert!(
        refusal.message.contains("`project:zephyr-quokka`"),
        "the refusal names the opt-in marker: {}",
        refusal.message
    );

    let text = extract_text(
        service
            .cas_rule_create(Parameters(create(Some("git,project:zephyr-quokka"))))
            .await
            .expect("an explicitly scoped rule is accepted"),
    );
    assert!(text.contains("Created rule"), "{text}");
}
