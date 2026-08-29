use cas::store::{
    MarkdownStore, RuleStore, SkillStore, SqliteRuleStore, SqliteSkillStore, SqliteStore, Store,
};
use cas::types::{Entry, Rule, Skill};
use tempfile::TempDir;

#[test]
fn sqlite_round_trips_entry_and_skill_source_ids() {
    let temp = TempDir::new().unwrap();
    let entry_store = SqliteStore::open(temp.path()).unwrap();
    entry_store.init().unwrap();
    let skill_store = SqliteSkillStore::open(temp.path()).unwrap();
    skill_store.init().unwrap();
    let rule_store = SqliteRuleStore::open(temp.path()).unwrap();
    rule_store.init().unwrap();

    let mut entry = Entry::new("entry-1".to_string(), "A derived learning".to_string());
    entry.source_ids = vec!["observation-1".to_string(), "observation-2".to_string()];
    entry_store.add(&entry).unwrap();
    assert_eq!(entry_store.get(&entry.id).unwrap().source_ids, entry.source_ids);

    let mut skill = Skill::new("skill-1".to_string(), "Derived skill".to_string());
    skill.source_ids = vec![entry.id.clone()];
    skill_store.add(&skill).unwrap();
    assert_eq!(skill_store.get(&skill.id).unwrap().source_ids, skill.source_ids);

    let mut rule = Rule::new("rule-1".to_string(), "Derived rule".to_string());
    rule.source_ids = vec![entry.id.clone()];
    rule_store.add(&rule).unwrap();
    assert_eq!(rule_store.get(&rule.id).unwrap().source_ids, rule.source_ids);

    let markdown_store = MarkdownStore::open(temp.path()).unwrap();
    markdown_store.init().unwrap();
    let mut markdown_entry = Entry::new(
        "markdown-entry-1".to_string(),
        "A markdown-derived learning".to_string(),
    );
    markdown_entry.source_ids = vec!["observation-3".to_string()];
    markdown_store.add(&markdown_entry).unwrap();
    assert_eq!(
        markdown_store.get(&markdown_entry.id).unwrap().source_ids,
        markdown_entry.source_ids
    );
}

#[test]
fn legacy_json_without_source_ids_defaults_to_empty() {
    let entry: Entry = serde_json::from_str(
        r#"{"id":"entry-1","created":"2026-08-29T00:00:00Z","content":"legacy"}"#,
    )
    .unwrap();
    assert!(entry.source_ids.is_empty());

    let skill: Skill = serde_json::from_str(
        r#"{"id":"skill-1","name":"legacy","description":"","skill_type":"command","invocation":"","created_at":"2026-08-29T00:00:00Z","updated_at":"2026-08-29T00:00:00Z"}"#,
    )
    .unwrap();
    assert!(skill.source_ids.is_empty());
}
