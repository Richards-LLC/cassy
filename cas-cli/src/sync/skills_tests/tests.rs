use crate::sync::skills::*;
use crate::types::Scope;
use tempfile::TempDir;

fn create_test_skill(name: &str, enabled: bool) -> Skill {
    Skill {
        source_ids: Vec::new(),
        id: format!("sk-{name}"),
        scope: Scope::default(),
        name: name.to_string(),
        description: format!("Test skill: {name}"),
        skill_type: crate::types::SkillType::Command,
        invocation: format!("Run: test-{name}"),
        parameters_schema: String::new(),
        example: format!("Example: test-{name} --help"),
        preconditions: Vec::new(),
        postconditions: Vec::new(),
        validation_script: String::new(),
        status: if enabled {
            SkillStatus::Enabled
        } else {
            SkillStatus::Disabled
        },
        tags: vec!["test".to_string()],
        summary: String::new(),
        invokable: false,
        argument_hint: String::new(),
        context_mode: None,
        agent_type: None,
        allowed_tools: Vec::new(),
        disallowed_tools: Vec::new(),
        hooks: None,
        disable_model_invocation: false,
        usage_count: 0,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
        last_used: None,
        team_id: None,
        share: None,
    }
}

/// cas-d731. `argument-hint: [title]` is not a string in YAML — the brackets
/// make it a flow sequence, so a real parser returned a one-element list where
/// every consumer expects text. The shared serializer quotes it, so this
/// asserts the value a parser actually yields rather than the raw bytes the
/// old escaper happened to write.
fn assert_frontmatter_string(content: &str, key: &str, expected: &str) {
    let frontmatter = content
        .split("---")
        .nth(1)
        .unwrap_or_else(|| panic!("no frontmatter in:\n{content}"));
    let parsed: serde_yaml::Value = serde_yaml::from_str(frontmatter)
        .unwrap_or_else(|e| panic!("frontmatter is not valid YAML: {e}\n{frontmatter}"));
    assert_eq!(
        parsed.get(key).and_then(|v| v.as_str()),
        Some(expected),
        "{key} must read back as the string {expected:?}:\n{frontmatter}"
    );
}

#[test]
fn test_is_enabled() {
    let syncer = SkillSyncer::new(PathBuf::from("/tmp/test"));

    let enabled = create_test_skill("enabled", true);
    let disabled = create_test_skill("disabled", false);

    assert!(syncer.is_enabled(&enabled));
    assert!(!syncer.is_enabled(&disabled));
}

#[test]
fn test_sync_skill() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let skill = create_test_skill("test-skill", true);

    // Sync the skill
    assert!(syncer.sync_skill(&skill).unwrap());

    // Check directory and file were created
    let skill_dir = target.join("cas-test-skill");
    assert!(skill_dir.exists());
    assert!(skill_dir.join("SKILL.md").exists());

    let content = fs::read_to_string(skill_dir.join("SKILL.md")).unwrap();
    assert!(content.contains("name: cas-test-skill"));
    assert!(content.contains("Test skill: test-skill"));
}

#[test]
fn test_sync_all() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let skill1 = create_test_skill("skill1", true);
    let skill2 = create_test_skill("skill2", false); // Not enabled

    let skills = vec![skill1, skill2];
    let report = syncer.sync_all(&skills).unwrap();

    assert_eq!(report.synced, 1);
    assert!(target.join("cas-skill1").exists());
    assert!(!target.join("cas-skill2").exists());
}

#[test]
fn test_remove_stale() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    // Create a stale skill directory
    let stale_dir = target.join("cas-stale");
    fs::create_dir_all(&stale_dir).unwrap();
    fs::write(stale_dir.join("SKILL.md"), "stale").unwrap();

    // Sync with no skills
    let report = syncer.sync_all(&[]).unwrap();

    assert_eq!(report.removed, 1);
    assert!(!stale_dir.exists());
}

#[test]
fn test_sanitize_name() {
    assert_eq!(sanitize_name("My Skill"), "my-skill");
    assert_eq!(sanitize_name("skill_test"), "skill-test");
    assert_eq!(sanitize_name("Skill-123"), "skill-123");
}

#[test]
fn test_sync_invokable_skill() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let mut skill = create_test_skill("my-task", true);
    skill.invokable = true;
    skill.argument_hint = "[title]".to_string();

    syncer.sync_skill(&skill).unwrap();

    let content = fs::read_to_string(target.join("cas-my-task/SKILL.md")).unwrap();
    assert_frontmatter_string(&content, "argument-hint", "[title]");
}

#[test]
fn test_sync_passive_skill_no_argument_hint() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let skill = create_test_skill("passive-skill", true);
    // invokable defaults to false

    syncer.sync_skill(&skill).unwrap();

    let content = fs::read_to_string(target.join("cas-passive-skill/SKILL.md")).unwrap();
    assert!(!content.contains("argument-hint"));
    // Non-invokable skills should have user-invocable: false
    assert!(content.contains("user-invocable: false"));
}

#[test]
fn test_sync_invokable_skill_no_user_invocable_false() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let mut skill = create_test_skill("invokable-skill", true);
    skill.invokable = true;
    skill.argument_hint = "[query]".to_string();

    syncer.sync_skill(&skill).unwrap();

    let content = fs::read_to_string(target.join("cas-invokable-skill/SKILL.md")).unwrap();
    // Invokable skills should NOT have user-invocable: false
    assert!(!content.contains("user-invocable: false"));
    assert_frontmatter_string(&content, "argument-hint", "[query]");
}

#[test]
fn test_sync_skill_with_context_fork() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let mut skill = create_test_skill("forked-skill", true);
    skill.context_mode = Some("fork".to_string());

    syncer.sync_skill(&skill).unwrap();

    let content = fs::read_to_string(target.join("cas-forked-skill/SKILL.md")).unwrap();
    assert!(content.contains("context: fork"));
}

#[test]
fn test_sync_skill_with_agent_type() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let mut skill = create_test_skill("agent-skill", true);
    skill.agent_type = Some("code-reviewer".to_string());

    syncer.sync_skill(&skill).unwrap();

    let content = fs::read_to_string(target.join("cas-agent-skill/SKILL.md")).unwrap();
    assert!(content.contains("agent: code-reviewer"));
}

#[test]
fn test_sync_skill_with_allowed_tools() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let mut skill = create_test_skill("restricted-skill", true);
    skill.allowed_tools = vec!["Read".to_string(), "Grep".to_string(), "Glob".to_string()];

    syncer.sync_skill(&skill).unwrap();

    let content = fs::read_to_string(target.join("cas-restricted-skill/SKILL.md")).unwrap();
    assert!(content.contains("allowed-tools:"));
    assert!(content.contains("  - Read"));
    assert!(content.contains("  - Grep"));
    assert!(content.contains("  - Glob"));
}

#[test]
fn test_sync_skill_with_all_frontmatter_fields() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let mut skill = create_test_skill("full-skill", true);
    skill.invokable = true;
    skill.argument_hint = "[file]".to_string();
    skill.context_mode = Some("fork".to_string());
    skill.agent_type = Some("Explore".to_string());
    skill.allowed_tools = vec!["Read".to_string(), "Bash".to_string()];
    skill.preconditions = vec![
        "git is installed".to_string(),
        "working tree is clean".to_string(),
    ];
    skill.postconditions = vec!["the requested files are formatted".to_string()];

    syncer.sync_skill(&skill).unwrap();

    let content = fs::read_to_string(target.join("cas-full-skill/SKILL.md")).unwrap();
    // Should have all frontmatter fields
    assert!(content.contains("name: cas-full-skill"));
    assert_frontmatter_string(&content, "argument-hint", "[file]");
    assert!(content.contains("context: fork"));
    assert!(content.contains("agent: Explore"));
    assert!(content.contains("allowed-tools:"));
    assert!(content.contains("  - Read"));
    assert!(content.contains("  - Bash"));
    assert!(content.contains("## Preconditions"));
    assert!(content.contains("- git is installed"));
    assert!(content.contains("## Postconditions"));
    assert!(content.contains("- the requested files are formatted"));
    // Should NOT have user-invocable: false since it IS invokable
    assert!(!content.contains("user-invocable: false"));
}

#[test]
fn test_create_cas_skill() {
    let temp = TempDir::new().unwrap();
    create_cas_skill(temp.path()).unwrap();

    let skill_file = temp.path().join(".claude/skills/cas/SKILL.md");
    assert!(skill_file.exists());

    let content = fs::read_to_string(skill_file).unwrap();
    assert!(content.contains("name: cas"));
    // Check for MCP mode content (always MCP-only now)
    assert!(
        content.contains("mcp__cas__memory"),
        "Should contain MCP memory tool reference"
    );
    assert!(
        content.contains("mcp__cas__task"),
        "Should contain MCP task tool reference"
    );
}

#[test]
fn test_sync_skill_with_disable_model_invocation() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let mut skill = create_test_skill("command-only", true);
    skill.disable_model_invocation = true;

    syncer.sync_skill(&skill).unwrap();

    let content = fs::read_to_string(target.join("cas-command-only/SKILL.md")).unwrap();
    assert!(content.contains("disable-model-invocation: true"));
}

#[test]
fn test_sync_skill_without_disable_model_invocation() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let skill = create_test_skill("normal-skill", true);
    // disable_model_invocation defaults to false

    syncer.sync_skill(&skill).unwrap();

    let content = fs::read_to_string(target.join("cas-normal-skill/SKILL.md")).unwrap();
    // Should not contain disable-model-invocation when false
    assert!(!content.contains("disable-model-invocation"));
}

#[test]
fn test_sync_skill_with_hooks() {
    use crate::types::{SkillHookConfig, SkillHookEntry, SkillHooks};

    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let mut skill = create_test_skill("hooked-skill", true);
    skill.hooks = Some(SkillHooks {
        pre_tool_use: None,
        post_tool_use: Some(vec![SkillHookConfig {
            matcher: Some("Write|Edit".to_string()),
            hooks: vec![SkillHookEntry {
                hook_type: "command".to_string(),
                command: "cas hook PostToolUse".to_string(),
                timeout: Some(5000),
            }],
        }]),
        stop: Some(vec![SkillHookConfig {
            matcher: None,
            hooks: vec![SkillHookEntry::new("cas hook Stop")],
        }]),
    });

    syncer.sync_skill(&skill).unwrap();

    let content = fs::read_to_string(target.join("cas-hooked-skill/SKILL.md")).unwrap();
    assert!(content.contains("hooks:"), "Missing hooks section");
    assert!(
        content.contains("PostToolUse:"),
        "Missing PostToolUse section"
    );
    assert!(
        content.contains("matcher: Write|Edit") || content.contains("matcher: \"Write|Edit\""),
        "Missing matcher - content:\n{content}"
    );
    assert!(
        content.contains("command: cas hook PostToolUse"),
        "Missing command"
    );
    assert!(content.contains("timeout: 5000"), "Missing timeout");
    assert!(content.contains("Stop:"), "Missing Stop section");
    assert!(
        content.contains("command: cas hook Stop"),
        "Missing Stop command"
    );
}

// cas-5be8: disallowed-tools frontmatter support
#[test]
fn test_sync_skill_with_disallowed_tools() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let mut skill = create_test_skill("blocked-skill", true);
    skill.disallowed_tools = vec!["Write".to_string(), "Edit".to_string(), "NotebookEdit".to_string()];

    syncer.sync_skill(&skill).unwrap();

    let content = fs::read_to_string(target.join("cas-blocked-skill/SKILL.md")).unwrap();
    assert!(content.contains("disallowed-tools:"), "disallowed-tools block must be emitted");
    assert!(content.contains("  - Write"), "Write must be listed");
    assert!(content.contains("  - Edit"), "Edit must be listed");
    assert!(content.contains("  - NotebookEdit"), "NotebookEdit must be listed");
}

#[test]
fn test_sync_skill_disallowed_tools_omitted_when_empty() {
    let temp = TempDir::new().unwrap();
    let target = temp.path().join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let skill = create_test_skill("open-skill", true);
    // disallowed_tools defaults to empty

    syncer.sync_skill(&skill).unwrap();

    let content = fs::read_to_string(target.join("cas-open-skill/SKILL.md")).unwrap();
    assert!(!content.contains("disallowed-tools:"), "disallowed-tools must NOT be emitted when empty");
}

/// cas-d731. The end-to-end claim: a skill whose summary contains the
/// characters that broke the old escaper is written to disk as frontmatter a
/// real YAML parser accepts, with the value intact. The reported defect —
/// `Use C:\project: inspect` — produced `description: "Use C:\project:
/// inspect"`, which a parser rejects with `found unknown escape character
/// 'p'`.
#[test]
fn a_synced_skill_writes_frontmatter_a_real_yaml_parser_accepts() {
    for summary in [
        "Use C:\\project: inspect",
        "Windows path C:\\Users\\dev and a colon: here",
        "quotes \" and ' together",
        "true",
        "- leading dash",
        "trailing space ",
        "multi\nline summary",
        "unicode — café 日本語 🎯",
    ] {
        let temp = TempDir::new().unwrap();
        let target = temp.path().join(".claude/skills");
        let syncer = SkillSyncer::new(target.clone());

        let mut skill = create_test_skill("hostile", true);
        skill.summary = summary.to_string();
        assert!(syncer.sync_skill(&skill).unwrap());

        let content = fs::read_to_string(target.join("cas-hostile/SKILL.md")).unwrap();
        let frontmatter = content
            .split("---")
            .nth(1)
            .unwrap_or_else(|| panic!("no frontmatter written for {summary:?}:\n{content}"));

        let parsed: serde_yaml::Value = serde_yaml::from_str(frontmatter).unwrap_or_else(|e| {
            panic!("generated frontmatter is not valid YAML for {summary:?}: {e}\n{frontmatter}")
        });
        assert_eq!(
            parsed.get("description").and_then(|v| v.as_str()),
            Some(summary),
            "the description changed on the way to disk:\n{frontmatter}"
        );

        // The frontmatter stays line-oriented: one `description:` line, no
        // block scalar continuation, so line-based readers still see the value.
        let description_lines = frontmatter
            .lines()
            .filter(|l| l.starts_with("description:"))
            .count();
        assert_eq!(description_lines, 1, "{frontmatter}");
    }
}

/// cas-d731. The other half of the round trip: what the writer puts on disk,
/// CAS's own reader must read back. External parser agreement is not enough —
/// the reader is line-oriented and decodes no escapes, so a writer fix that
/// ignored it would trade an invalid file for a silently wrong value.
#[test]
fn cas_reads_back_the_hostile_values_it_wrote() {
    let temp = TempDir::new().unwrap();
    let project = temp.path();
    let target = project.join(".claude/skills");
    let syncer = SkillSyncer::new(target.clone());

    let cases = [
        ("windows", "Use C:\\project: inspect"),
        ("quotes", "quotes \" and ' together"),
        ("implicit", "true"),
        ("dash", "- leading dash"),
        ("newline", "multi\nline summary"),
        ("unicode", "unicode — café 日本語 🎯"),
    ];
    for (name, summary) in cases {
        let mut skill = create_test_skill(name, true);
        skill.summary = summary.to_string();
        assert!(syncer.sync_skill(&skill).unwrap());
    }

    let read_back = read_skills_from_files(project).unwrap();
    for (name, summary) in cases {
        let found = read_back
            .iter()
            .find(|s| s.name == format!("cas-{name}"))
            .unwrap_or_else(|| {
                panic!(
                    "CAS could not read back the skill it wrote for {summary:?}; got {:?}",
                    read_back.iter().map(|s| &s.name).collect::<Vec<_>>()
                )
            });
        assert_eq!(
            found.summary, summary,
            "CAS's own reader changed the value it had just written for {name}"
        );
    }
}

/// The narrow legacy fallback, justified by a real fixture rather than by
/// caution: this is exactly what the old escaper wrote for the reported
/// defect, and it is not valid YAML. Files like it exist in every checkout
/// that ran a sync before the fix, so the reader must still understand them.
#[test]
fn a_legacy_invalid_yaml_skill_file_is_still_readable() {
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join(".claude/skills/cas-legacy");
    fs::create_dir_all(&skill_dir).unwrap();
    // Verbatim pre-cas-d731 output: the raw backslash makes this invalid YAML.
    let legacy = "---\nname: cas-legacy\ndescription: \"Use C:\\project: inspect\"\n---\n\nBody.\n";
    assert!(
        serde_yaml::from_str::<serde_yaml::Value>(
            legacy.split("---").nth(1).unwrap()
        )
        .is_err(),
        "this fixture is only meaningful if a parser really rejects it"
    );
    fs::write(skill_dir.join("SKILL.md"), legacy).unwrap();

    let read_back = read_skills_from_files(temp.path()).unwrap();
    let skill = read_back
        .iter()
        .find(|s| s.name == "cas-legacy")
        .expect("a legacy file must not become unreadable");
    assert!(
        skill.summary.contains("Use C:"),
        "the legacy scan must still recover the description, got {:?}",
        skill.summary
    );
}

/// A document that parses but omits the key returns None: the parser's answer
/// stands, and the legacy scan must not be used to invent a value.
#[test]
fn valid_yaml_without_the_key_does_not_fall_back_to_scanning() {
    let temp = TempDir::new().unwrap();
    let skill_dir = temp.path().join(".claude/skills/cas-nodesc");
    fs::create_dir_all(&skill_dir).unwrap();
    // `description_extra` would satisfy the legacy prefix scan for the key
    // `description`; a parsed document must not be reinterpreted that way.
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: cas-nodesc\ndescription_extra: leaked\n---\n\nBody.\n",
    )
    .unwrap();

    let read_back = read_skills_from_files(temp.path()).unwrap();
    let skill = read_back
        .iter()
        .find(|s| s.name == "cas-nodesc")
        .expect("the file itself is valid and must still be read");
    assert_eq!(
        skill.summary, "",
        "a missing key must read as absent, not as a prefix match"
    );
}
