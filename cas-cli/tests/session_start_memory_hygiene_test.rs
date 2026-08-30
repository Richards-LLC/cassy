use assert_cmd::Command;
use cas::hooks::build_context_with_token_budget;
use cas::types::{Entry, EntryType};
use cas_core::hooks::context::{ContextStores, build_context_with_stores};
use cas_core::hooks::{DefaultHooksConfig, HookInput};
use cas_core::memory::{contamination_patterns, find_contaminated_entries};
use cas_store::{
    RuleStore, SkillStore, SqliteRuleStore, SqliteSkillStore, SqliteStore,
    SqliteSurfacedArtifactStore, Store,
};
use cas_types::{MemoryTier, Rule, RuleStatus, Session, SessionOutcome, Skill, SkillStatus};
use std::fs;

fn session_start_input() -> HookInput {
    HookInput {
        cwd: "/project".to_string(),
        hook_event_name: "SessionStart".to_string(),
        ..Default::default()
    }
}

#[test]
fn cli_session_start_persists_surface_ledger_for_injected_rules_and_skills() {
    let temp = tempfile::tempdir().unwrap();
    let cas_root = temp.path().join(".cas");
    fs::create_dir_all(&cas_root).unwrap();

    let entry_store = SqliteStore::open(&cas_root).unwrap();
    entry_store.init().unwrap();
    let session = Session::new("surface-session".to_string(), "/project".to_string(), None);
    entry_store.start_session(&session).unwrap();

    let rule_store = SqliteRuleStore::open(&cas_root).unwrap();
    rule_store.init().unwrap();
    let mut rule = Rule::new("surface-rule".to_string(), "Surface this rule".to_string());
    rule.status = RuleStatus::Proven;
    rule_store.add(&rule).unwrap();

    let skill_store = SqliteSkillStore::open(&cas_root).unwrap();
    skill_store.init().unwrap();
    let mut skill = Skill::new(
        "surface-skill".to_string(),
        "Surface this skill".to_string(),
    );
    skill.status = SkillStatus::Enabled;
    skill_store.add(&skill).unwrap();

    let input = HookInput {
        session_id: "surface-session".to_string(),
        cwd: "/project".to_string(),
        hook_event_name: "SessionStart".to_string(),
        ..Default::default()
    };
    let context = build_context_with_token_budget(&input, 10, &cas_root, None).unwrap();
    assert!(context.contains("Surface this rule"));
    assert!(context.contains("Surface this skill"));

    let surface_store = SqliteSurfacedArtifactStore::open(&cas_root).unwrap();
    assert_eq!(
        surface_store.count_for_session("surface-session").unwrap(),
        2
    );
    assert_eq!(rule_store.get("surface-rule").unwrap().surface_count, 1);
    entry_store
        .update_session_outcome("surface-session", SessionOutcome::TasksCompleted)
        .unwrap();
    let impacts = surface_store.aggregate(10).unwrap();
    assert_eq!(
        impacts
            .iter()
            .find(|impact| impact.artifact_id == "surface-rule")
            .unwrap()
            .outcome_counts["tasks_completed"],
        1
    );
}

#[test]
fn high_importance_preference_injects_its_full_first_line() {
    let first_line = "Slack embargo: do not post internal release details until the operator explicitly clears the embargo; this standing instruction must remain visible in every session";
    let entry = Entry {
        id: "2026-08-14-14".to_string(),
        entry_type: EntryType::Preference,
        importance: 1.0,
        content: format!(
            "{first_line}\nA second line that is not part of the operative first-line preview."
        ),
        ..Entry::new("unused".to_string(), String::new())
    };

    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path()).unwrap();
    store.init().unwrap();
    store.add(&entry).unwrap();

    let config = DefaultHooksConfig::new();
    let stores = ContextStores {
        project_store: Some(&store),
        ..ContextStores::empty()
    };
    let (context, stats) = build_context_with_stores(
        &session_start_input(),
        &stores,
        &config,
        10,
        None,
        "mcp__cas__",
    )
    .unwrap();

    assert_eq!(stats.memories_included, 1, "preference was not surfaced");
    assert!(
        context.contains(first_line),
        "high-importance preference was truncated in SessionStart:\n{context}"
    );
}

#[test]
fn helpful_memories_only_surface_active_feedback_eligible_entries() {
    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path()).unwrap();
    store.init().unwrap();

    let working = Entry::new(
        "helpful-working".to_string(),
        "working memory eligible for helpful memories".to_string(),
    );
    let archived_tier = Entry {
        id: "helpful-archive-tier".to_string(),
        memory_tier: MemoryTier::Archive,
        content: "archive tier must stay out of helpful memories".to_string(),
        ..Entry::new("unused-archive-tier".to_string(), String::new())
    };
    let raw_context = Entry {
        id: "raw-context".to_string(),
        entry_type: EntryType::Context,
        content: "raw context blob without feedback must stay out".to_string(),
        ..Entry::new("unused-raw-context".to_string(), String::new())
    };
    let feedback_context = Entry {
        id: "feedback-context".to_string(),
        entry_type: EntryType::Context,
        helpful_count: 1,
        content: "context with helpful feedback remains eligible".to_string(),
        ..Entry::new("unused-feedback-context".to_string(), String::new())
    };

    for entry in [working, archived_tier, raw_context, feedback_context] {
        store.add(&entry).unwrap();
    }

    let config = DefaultHooksConfig::new().with_token_budget(2_000);
    let stores = ContextStores {
        project_store: Some(&store),
        ..ContextStores::empty()
    };
    let (context, stats) = build_context_with_stores(
        &session_start_input(),
        &stores,
        &config,
        10,
        None,
        "mcp__cas__",
    )
    .unwrap();

    assert_eq!(stats.memories_included, 2);
    assert!(context.contains("helpful-working"));
    assert!(context.contains("feedback-context"));
    assert!(!context.contains("helpful-archive-tier"));
    assert!(!context.contains("raw-context"));
}

#[test]
fn contamination_scan_reports_tool_call_artifacts_without_mutating_entries() {
    let clean = Entry::new("clean".to_string(), "ordinary memory prose".to_string());
    let dirty = Entry::new(
        "dirty".to_string(),
        "stored response\n</invoke>\nkeep the surrounding prose".to_string(),
    );

    assert_eq!(contamination_patterns(&clean.content), Vec::<&str>::new());
    assert_eq!(contamination_patterns(&dirty.content), vec!["</invoke>"]);
    let findings = find_contaminated_entries(&[clean.clone(), dirty.clone()]);
    assert_eq!(
        findings,
        vec![cas_core::memory::ContaminatedEntry {
            id: dirty.id.clone(),
            patterns: vec!["</invoke>"],
        }]
    );
    assert_eq!(
        dirty.content,
        "stored response\n</invoke>\nkeep the surrounding prose"
    );
}

#[test]
fn high_importance_preferences_are_kept_when_the_memory_budget_omits_other_entries() {
    let first_line = "Standing preference: keep the release embargo active until the operator explicitly clears it for this session.";
    let important = Entry {
        id: "important".to_string(),
        entry_type: EntryType::Preference,
        importance: 1.0,
        content: format!("{first_line}\nAdditional context that is not part of the first line."),
        ..Entry::new("unused-important".to_string(), String::new())
    };
    let ordinary = Entry::new(
        "ordinary".to_string(),
        "ordinary memory that should be the first item omitted when the compact memory budget is tight".to_string(),
    );

    let temp = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(temp.path()).unwrap();
    store.init().unwrap();
    store.add(&ordinary).unwrap();
    store.add(&important).unwrap();

    let config = DefaultHooksConfig::new().with_token_budget(350);
    let stores = ContextStores {
        project_store: Some(&store),
        ..ContextStores::empty()
    };
    let (context, stats) = build_context_with_stores(
        &session_start_input(),
        &stores,
        &config,
        10,
        None,
        "mcp__cas__",
    )
    .unwrap();

    assert_eq!(
        stats.memories_included, 1,
        "expected one memory within budget"
    );
    assert!(
        context.contains(first_line),
        "important preference was cut:\n{context}"
    );
    assert!(
        !context.contains("ordinary memory that should"),
        "ordinary memory was not omitted:\n{context}"
    );
    assert!(
        context.contains("+1 more"),
        "budget cut was not disclosed:\n{context}"
    );
    assert!(
        stats.total_tokens <= config.token_budget,
        "context exceeded budget: {stats:?}"
    );
}

#[test]
fn hygiene_cli_reports_findings_without_rewriting_the_store() {
    let project = tempfile::tempdir().unwrap();
    let cas_root = project.path().join(".cas");
    let home = project.path().join("home");
    let xdg = project.path().join("xdg");
    fs::create_dir_all(&cas_root).unwrap();
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&xdg).unwrap();

    let entry = Entry::new("dirty".to_string(), "persisted\n</invoke>".to_string());
    let store = SqliteStore::open(&cas_root).unwrap();
    store.init().unwrap();
    store.add(&entry).unwrap();

    let mut command = Command::new(cas::test_paths::cas_binary());
    let output = command
        .current_dir(project.path())
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &xdg)
        .env_remove("CAS_ROOT")
        .args(["memory", "hygiene"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "hygiene command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("dirty"), "finding was not listed: {stdout}");
    assert!(
        stdout.contains("</invoke>"),
        "matched pattern was not listed: {stdout}"
    );
    assert_eq!(store.get("dirty").unwrap().content, entry.content);
}
