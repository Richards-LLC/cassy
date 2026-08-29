use crate::mcp::tools::types::defaults::*;
use crate::mcp::tools::types::*;

// =========================================================================
// Default value tests
// =========================================================================

#[test]
fn test_default_entry_type() {
    assert_eq!(default_entry_type(), "learning");
}

#[test]
fn test_default_importance() {
    assert_eq!(default_importance(), 0.5);
}

#[test]
fn test_default_recent() {
    assert_eq!(default_recent(), 10);
}

#[test]
fn test_default_priority() {
    assert_eq!(default_priority(), 2);
}

#[test]
fn test_default_task_type() {
    assert_eq!(default_task_type(), "task");
}

#[test]
fn test_default_search_limit() {
    assert_eq!(default_search_limit(), 10);
}

#[test]
fn test_default_observation_type() {
    assert_eq!(default_observation_type(), "general");
}

#[test]
fn test_default_skill_type() {
    assert_eq!(default_skill_type(), "command");
}

#[test]
fn test_default_dep_type() {
    assert_eq!(default_dep_type(), "blocks");
}

#[test]
fn test_default_note_type() {
    assert_eq!(default_note_type(), "progress");
}

#[test]
fn test_default_subagent_tokens() {
    assert_eq!(default_subagent_tokens(), 2000);
}

#[test]
fn test_default_scope_project() {
    assert_eq!(default_scope_project(), "project");
}

#[test]
fn test_default_scope_global() {
    assert_eq!(default_scope_global(), "global");
}

#[test]
fn test_default_scope_all() {
    assert_eq!(default_scope_all(), "all");
}

// =========================================================================
// Deserialization tests with defaults
// =========================================================================

#[test]
fn test_remember_request_defaults() {
    let json = r#"{"content": "Test content"}"#;
    let req: RememberRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.content, "Test content");
    assert_eq!(req.entry_type, "learning");
    assert_eq!(req.importance, 0.5);
    assert_eq!(req.scope, "project"); // Default scope
    assert!(req.tags.is_none());
    assert!(req.title.is_none());
}

#[test]
fn test_remember_request_full() {
    let json = r#"{
        "content": "Important fact",
        "entry_type": "preference",
        "tags": "rust,cli",
        "title": "CLI Preference",
        "importance": 0.9,
        "scope": "global"
    }"#;
    let req: RememberRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.entry_type, "preference");
    assert_eq!(req.importance, 0.9);
    assert_eq!(req.scope, "global");
    assert_eq!(req.tags, Some("rust,cli".to_string()));
    assert_eq!(req.title, Some("CLI Preference".to_string()));
}

#[test]
fn test_task_create_request_defaults() {
    let json = r#"{"title": "Fix bug"}"#;
    let req: TaskCreateRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.title, "Fix bug");
    assert_eq!(req.priority, 2);
    assert_eq!(req.task_type, "task");
    assert!(req.description.is_none());
    assert!(req.labels.is_none());
    assert!(req.notes.is_none());
    assert!(req.blocked_by.is_none());
}

#[test]
fn test_task_create_request_full() {
    let json = r#"{
        "title": "Implement feature",
        "description": "Add new login flow",
        "priority": 1,
        "task_type": "feature",
        "labels": "auth,ui",
        "notes": "Started planning",
        "blocked_by": "cas-1234"
    }"#;
    let req: TaskCreateRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.title, "Implement feature");
    assert_eq!(req.priority, 1);
    assert_eq!(req.task_type, "feature");
    assert_eq!(req.description, Some("Add new login flow".to_string()));
    assert_eq!(req.blocked_by, Some("cas-1234".to_string()));
}

#[test]
fn test_search_request_defaults() {
    let json = r#"{"query": "rust async"}"#;
    let req: SearchRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.query, "rust async");
    assert_eq!(req.limit, 10);
    assert_eq!(req.scope, "all"); // Default scope for search
    assert!(req.doc_type.is_none());
}

#[test]
fn test_search_request_with_filter() {
    let json = r#"{"query": "database", "limit": 5, "doc_type": "task"}"#;
    let req: SearchRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.limit, 5);
    assert_eq!(req.doc_type, Some("task".to_string()));
}

#[test]
fn test_observe_request_defaults() {
    let json = r#"{"content": "Fixed the parser bug"}"#;
    let req: ObserveRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.content, "Fixed the parser bug");
    assert_eq!(req.observation_type, "general");
    assert_eq!(req.scope, "project"); // Default scope
    assert!(req.source_tool.is_none());
    assert!(req.tags.is_none());
}

#[test]
fn test_rule_create_request() {
    let json = r#"{"content": "Use async/await", "paths": "src/**/*.rs", "tags": "rust"}"#;
    let req: RuleCreateRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.content, "Use async/await");
    assert_eq!(req.paths, Some("src/**/*.rs".to_string()));
    assert_eq!(req.tags, Some("rust".to_string()));
    assert!(req.source_ids.is_none());
}

#[test]
fn test_rule_and_skill_create_request_source_ids() {
    let rule: RuleCreateRequest = serde_json::from_str(
        r#"{"content":"Use async/await","source_ids":"entry-1,entry-2"}"#,
    )
    .unwrap();
    assert_eq!(rule.source_ids.as_deref(), Some("entry-1,entry-2"));

    let skill: SkillCreateRequest = serde_json::from_str(
        r#"{"name":"Format","description":"Run fmt","invocation":"cargo fmt","source_ids":"entry-3"}"#,
    )
    .unwrap();
    assert_eq!(skill.source_ids.as_deref(), Some("entry-3"));
}

#[test]
fn test_skill_create_request_defaults() {
    let json = r#"{
        "name": "Format Code",
        "description": "Run cargo fmt",
        "invocation": "cargo fmt"
    }"#;
    let req: SkillCreateRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.name, "Format Code");
    assert_eq!(req.skill_type, "command");
    assert_eq!(req.scope, "global"); // Default scope for skills
    assert!(req.tags.is_none());
    assert!(req.source_ids.is_none());
}

#[test]
fn test_dependency_request_defaults() {
    let json = r#"{"from_id": "cas-1", "to_id": "cas-2"}"#;
    let req: DependencyRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.from_id, "cas-1");
    assert_eq!(req.to_id, "cas-2");
    assert_eq!(req.dep_type, "blocks");
}

#[test]
fn test_task_show_request_defaults() {
    let json = r#"{"id": "cas-1234"}"#;
    let req: TaskShowRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.id, "cas-1234");
    assert!(req.with_deps);
}

#[test]
fn test_task_notes_request_defaults() {
    let json = r#"{"id": "cas-1234", "note": "Made progress"}"#;
    let req: TaskNotesRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.id, "cas-1234");
    assert_eq!(req.note, "Made progress");
    assert_eq!(req.note_type, "progress");
}

#[test]
fn test_task_notes_request_with_type() {
    let json = r#"{"id": "cas-1234", "note": "API is slow", "note_type": "blocker"}"#;
    let req: TaskNotesRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.note_type, "blocker");
}

#[test]
fn test_subagent_context_request_defaults() {
    let json = r#"{"task_id": "cas-1234"}"#;
    let req: SubAgentContextRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.task_id, "cas-1234");
    assert_eq!(req.max_tokens, 2000);
    assert!(req.include_memories);
}

#[test]
fn test_reindex_request_defaults() {
    let json = r#"{}"#;
    let req: ReindexRequest = serde_json::from_str(json).unwrap();
    assert!(!req.bm25);
    assert!(!req.embeddings);
    assert!(!req.missing_only);
}

#[test]
fn test_reindex_request_full() {
    let json = r#"{"bm25": true, "embeddings": true, "missing_only": true}"#;
    let req: ReindexRequest = serde_json::from_str(json).unwrap();
    assert!(req.bm25);
    assert!(req.embeddings);
    assert!(req.missing_only);
}

#[test]
fn test_maintenance_run_request_defaults() {
    let json = r#"{}"#;
    let req: MaintenanceRunRequest = serde_json::from_str(json).unwrap();
    assert!(!req.force);
}

#[test]
fn test_id_request() {
    let json = r#"{"id": "2024-01-15-001"}"#;
    let req: IdRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.id, "2024-01-15-001");
}

#[test]
fn test_limit_request_defaults() {
    let json = r#"{}"#;
    let req: LimitRequest = serde_json::from_str(json).unwrap();
    assert!(req.limit.is_none());
    assert_eq!(req.scope, "all"); // Default scope for list
}

#[test]
fn test_limit_request_with_value() {
    let json = r#"{"limit": 25}"#;
    let req: LimitRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.limit, Some(25));
}

#[test]
fn test_memory_tier_request() {
    let json = r#"{"id": "2024-01-15-001", "tier": "cold"}"#;
    let req: MemoryTierRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.id, "2024-01-15-001");
    assert_eq!(req.tier, "cold");
}

#[test]
fn test_opinion_reinforce_request() {
    let json = r#"{"id": "2024-01-15-001", "evidence": "Performance tests confirmed"}"#;
    let req: OpinionReinforceRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.id, "2024-01-15-001");
    assert_eq!(req.evidence, "Performance tests confirmed");
}

#[test]
fn test_opinion_weaken_request() {
    let json = r#"{"id": "2024-01-15-001", "evidence": "Edge case found"}"#;
    let req: OpinionWeakenRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.id, "2024-01-15-001");
    assert_eq!(req.evidence, "Edge case found");
}

#[test]
fn test_opinion_contradict_request() {
    let json = r#"{"id": "2024-01-15-001", "evidence": "Completely wrong approach"}"#;
    let req: OpinionContradictRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.id, "2024-01-15-001");
    assert_eq!(req.evidence, "Completely wrong approach");
}
