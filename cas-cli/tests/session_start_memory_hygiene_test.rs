use cas::types::{Entry, EntryType};
use cas_core::hooks::context::{ContextStores, build_context_with_stores};
use cas_core::hooks::{DefaultHooksConfig, HookInput};
use cas_core::memory::{contamination_patterns, find_contaminated_entries};
use cas_store::{SqliteStore, Store};

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
        content: format!("{first_line}\nA second line that is not part of the operative first-line preview."),
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
    assert_eq!(
        contamination_patterns(&dirty.content),
        vec!["</invoke>"]
    );
    let findings = find_contaminated_entries(&[clean.clone(), dirty.clone()]);
    assert_eq!(
        findings,
        vec![cas_core::memory::ContaminatedEntry {
            id: dirty.id.clone(),
            patterns: vec!["</invoke>"],
        }]
    );
    assert_eq!(dirty.content, "stored response\n</invoke>\nkeep the surrounding prose");
}
