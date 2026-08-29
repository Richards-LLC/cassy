use assert_cmd::Command;
use cas::types::{Entry, EntryType};
use cas_core::hooks::context::{ContextStores, build_context_with_stores};
use cas_core::hooks::{DefaultHooksConfig, HookInput};
use cas_core::memory::{contamination_patterns, find_contaminated_entries};
use cas_store::{SqliteStore, Store};
use std::fs;

fn session_start_input() -> HookInput {
    HookInput {
        cwd: "/project".to_string(),
        hook_event_name: "SessionStart".to_string(),
        ..Default::default()
    }
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
