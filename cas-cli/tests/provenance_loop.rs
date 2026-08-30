use cas::sync::{SkillSyncer, read_skill_from_file};
use cas::types::{Scope, Skill, SkillStatus, SkillType};
use tempfile::TempDir;

#[test]
fn synced_skill_file_round_trips_provenance() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target);
    let now = chrono::Utc::now();
    let skill = Skill {
        id: "skill-provenance".to_string(),
        scope: Scope::Project,
        name: "Provenance Skill".to_string(),
        description: "A skill derived from reviewed knowledge".to_string(),
        skill_type: SkillType::Command,
        invocation: "cargo test".to_string(),
        parameters_schema: String::new(),
        example: String::new(),
        preconditions: Vec::new(),
        postconditions: Vec::new(),
        validation_script: String::new(),
        status: SkillStatus::Enabled,
        tags: vec!["from_learning".to_string()],
        summary: "Use the reviewed test workflow".to_string(),
        invokable: false,
        argument_hint: String::new(),
        context_mode: None,
        agent_type: None,
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
        hooks: None,
        disable_model_invocation: false,
        usage_count: 0,
        created_at: now,
        updated_at: now,
        last_used: None,
        team_id: None,
        source_ids: vec!["learning-1".to_string(), "rule-2".to_string()],
        share: None,
    };

    assert!(syncer.sync_skill(&skill).unwrap());
    let project_root = temp.path();
    let parsed = read_skill_from_file(project_root, "cas-provenance-skill")
        .unwrap()
        .expect("synced skill should be readable");

    assert_eq!(parsed.source_ids, skill.source_ids);
}
