use crate::Store;
use crate::sqlite::SqliteStore;
use cas_types::{Entry, Scope, ShareScope};
use chrono::{Duration, Utc};
use tempfile::TempDir;

// Imports used only by the per-entity round-trip tests below.
use crate::{RuleStore, SkillStore, TaskStore};
use crate::{SqliteRuleStore, SqliteSkillStore, SqliteTaskStore};
use cas_types::{Rule, Skill, Task};

#[test]
fn test_sqlite_store_crud() {
    let temp = TempDir::new().unwrap();
    let store = SqliteStore::open(temp.path()).unwrap();
    store.init().unwrap();

    // Generate ID and add entry
    let id = store.generate_id().unwrap();
    let entry = Entry {
        id: id.clone(),
        content: "Test content".to_string(),
        ..Default::default()
    };
    store.add(&entry).unwrap();

    // Get entry
    let retrieved = store.get(&id).unwrap();
    assert_eq!(retrieved.content, "Test content");

    // Update entry
    let mut updated = retrieved;
    updated.content = "Updated content".to_string();
    updated.helpful_count = 5;
    store.update(&updated).unwrap();

    let retrieved = store.get(&id).unwrap();
    assert_eq!(retrieved.content, "Updated content");
    assert_eq!(retrieved.helpful_count, 5);

    // List entries
    let entries = store.list().unwrap();
    assert_eq!(entries.len(), 1);

    // Archive entry
    store.archive(&id).unwrap();
    assert!(store.get(&id).is_err());

    let archived = store.list_archived().unwrap();
    assert_eq!(archived.len(), 1);

    // Unarchive entry
    store.unarchive(&id).unwrap();
    assert!(store.get(&id).is_ok());

    // Delete entry
    store.delete(&id).unwrap();
    assert!(store.get(&id).is_err());
}

/// A cloud-synced row can be present without the local daily sequence having
/// advanced past it.  The next locally-generated ID must skip every existing
/// ID for today, not retry the stale sequence value indefinitely.
#[test]
fn test_generate_id_skips_foreign_same_day_entries_after_stale_sequence() {
    let temp = TempDir::new().unwrap();
    let store = SqliteStore::open(temp.path()).unwrap();
    store.init().unwrap();

    let today = Utc::now().format("%Y-%m-%d").to_string();
    for suffix in [16, 38, 41] {
        let entry = Entry::new(format!("{today}-{suffix}"), "synced elsewhere".to_string());
        store.add(&entry).unwrap();
    }

    // Simulate a restored/local sequence that has not observed the synced
    // entries. The old implementation returned `today-38`, colliding with a
    // row above and reproducing the live `remember` failure.
    let sequence_name = format!("entry:{today}");
    store.generate_id().unwrap(); // lazily creates the sequence table
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "INSERT INTO id_sequences (name, next_val) VALUES (?1, 37)
             ON CONFLICT(name) DO UPDATE SET next_val = excluded.next_val",
            [&sequence_name],
        )
        .unwrap();

    let id = store.generate_id().unwrap();
    assert_eq!(id, format!("{today}-42"));
    store
        .add(&Entry::new(id, "new local memory".to_string()))
        .unwrap();
}

#[test]
fn test_duplicate_entry_error_names_the_colliding_id() {
    let temp = TempDir::new().unwrap();
    let store = SqliteStore::open(temp.path()).unwrap();
    store.init().unwrap();

    let entry = Entry::new(
        "synced-collision".to_string(),
        "existing memory".to_string(),
    );
    store.add(&entry).unwrap();

    let error = store.add(&entry).unwrap_err();
    assert_eq!(error.to_string(), "entry already exists: synced-collision");
}

#[test]
fn test_recent_resurfaces_an_old_entry_updated_later() {
    let temp = TempDir::new().unwrap();
    let store = SqliteStore::open(temp.path()).unwrap();
    store.init().unwrap();

    let old = Entry {
        id: "p-old-handoff".to_string(),
        created: Utc::now() - Duration::days(30),
        content: "old handoff".to_string(),
        ..Default::default()
    };
    store.add(&old).unwrap();

    let newer = Entry {
        id: "p-newer-memory".to_string(),
        created: Utc::now() - Duration::days(1),
        content: "newer memory".to_string(),
        ..Default::default()
    };
    store.add(&newer).unwrap();

    std::thread::sleep(std::time::Duration::from_millis(2));
    let mut updated_old = store.get(&old.id).unwrap();
    updated_old.content.push_str(" with latest session notes");
    store.update(&updated_old).unwrap();

    let recent = store.recent(2).unwrap();
    assert_eq!(
        recent
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        vec!["p-old-handoff", "p-newer-memory"]
    );
    assert!(store.recent_timestamp(&recent[0]).unwrap() > recent[0].created);
}

#[test]
fn test_list_by_scope_and_tag_filters_in_sqlite_store() {
    let temp = TempDir::new().unwrap();
    let store = SqliteStore::open(temp.path()).unwrap();
    store.init().unwrap();

    let mut global_match = Entry::with_scope(
        "g-host-match".to_string(),
        "same host global".to_string(),
        Scope::Global,
    );
    global_match.tags = vec!["host:soundwave".to_string(), "tmpfs".to_string()];
    store.add(&global_match).unwrap();

    let mut project_match = Entry::with_scope(
        "p-host-match".to_string(),
        "same host project".to_string(),
        Scope::Project,
    );
    project_match.tags = vec!["host:soundwave".to_string()];
    store.add(&project_match).unwrap();

    let mut global_other_host = Entry::with_scope(
        "g-other-host".to_string(),
        "different host global".to_string(),
        Scope::Global,
    );
    global_other_host.tags = vec!["host:other".to_string()];
    store.add(&global_other_host).unwrap();

    let global = store
        .list_by_scope_and_tag(Scope::Global, "host:soundwave")
        .unwrap();
    let ids = global
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["g-host-match"]);

    let project = store
        .list_by_scope_and_tag(Scope::Project, "host:soundwave")
        .unwrap();
    let ids = project
        .iter()
        .map(|entry| entry.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(ids, vec!["p-host-match"]);
}

/// T5 cas-07d7: Entry.share must round-trip through SQLite.
/// Covers the three states — None, Private, Team — across add→get
/// and update→get, plus archive→unarchive (the P1 residual where
/// reload was losing share on the rule path).
#[test]
fn test_entry_share_roundtrip() {
    let temp = TempDir::new().unwrap();
    let store = SqliteStore::open(temp.path()).unwrap();
    store.init().unwrap();

    // None (default) round-trips
    let id_none = store.generate_id().unwrap();
    let e_none = Entry {
        id: id_none.clone(),
        content: "no share".to_string(),
        ..Default::default()
    };
    store.add(&e_none).unwrap();
    assert_eq!(store.get(&id_none).unwrap().share, None);

    // Some(Team) round-trips through add
    let id_team = store.generate_id().unwrap();
    let e_team = Entry {
        id: id_team.clone(),
        content: "team share".to_string(),
        share: Some(ShareScope::Team),
        ..Default::default()
    };
    store.add(&e_team).unwrap();
    assert_eq!(store.get(&id_team).unwrap().share, Some(ShareScope::Team));

    // Some(Private) round-trips through update
    let mut e_priv = store.get(&id_none).unwrap();
    e_priv.share = Some(ShareScope::Private);
    store.update(&e_priv).unwrap();
    assert_eq!(
        store.get(&id_none).unwrap().share,
        Some(ShareScope::Private)
    );

    // Archive/unarchive preserves share (P1 residual from T3 review)
    store.archive(&id_team).unwrap();
    store.unarchive(&id_team).unwrap();
    assert_eq!(store.get(&id_team).unwrap().share, Some(ShareScope::Team));
}

/// T5 cas-07d7: Rule.share must round-trip. The rule store's INSERT/
/// UPDATE/SELECT paths were all extended to cover `share` — this test
/// guards against future column-list drift.
#[test]
fn test_rule_share_roundtrip() {
    let temp = TempDir::new().unwrap();
    let store = SqliteRuleStore::open(temp.path()).unwrap();
    store.init().unwrap();

    let mut r = Rule::new("rule-001".to_string(), "test".to_string());
    r.share = Some(ShareScope::Team);
    store.add(&r).unwrap();
    assert_eq!(store.get("rule-001").unwrap().share, Some(ShareScope::Team));

    // Update round-trip
    let mut loaded = store.get("rule-001").unwrap();
    loaded.share = Some(ShareScope::Private);
    store.update(&loaded).unwrap();
    assert_eq!(
        store.get("rule-001").unwrap().share,
        Some(ShareScope::Private)
    );

    // None round-trip
    let mut loaded = store.get("rule-001").unwrap();
    loaded.share = None;
    store.update(&loaded).unwrap();
    assert_eq!(store.get("rule-001").unwrap().share, None);
}

/// cas-30af: updates must retain the prior rule snapshot and deletes must
/// leave a queryable retired row instead of destroying it.
#[test]
fn test_rule_history_and_tombstone_delete() {
    let temp = TempDir::new().unwrap();
    let store = SqliteRuleStore::open(temp.path()).unwrap();
    store.init().unwrap();

    let rule = Rule::new("rule-history-01".to_string(), "original content".to_string());
    store.add(&rule).unwrap();
    let mut updated = rule.clone();
    updated.content = "updated content".to_string();
    store
        .update_with_metadata(&updated, Some("test-actor"), Some("revise wording"))
        .unwrap();

    let versions = store.list_versions("rule-history-01").unwrap();
    assert_eq!(versions.len(), 1);
    assert_eq!(versions[0].content, "original content");
    assert_eq!(versions[0].changed_by.as_deref(), Some("test-actor"));
    assert_eq!(versions[0].change_note, "revise wording");

    let conn = rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
    let prior_content: String = conn
        .query_row(
            "SELECT content FROM rule_versions WHERE rule_id = 'rule-history-01' ORDER BY id DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(prior_content, "original content");

    store.delete("rule-history-01").unwrap();
    let retired = store.get("rule-history-01").unwrap();
    assert_eq!(retired.status.to_string(), "retired");
    let version_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM rule_versions WHERE rule_id = 'rule-history-01'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(version_count, 2);

    store
        .restore_version("rule-history-01", Some(1), Some("restorer"), None)
        .unwrap();
    let restored = store.get("rule-history-01").unwrap();
    assert_eq!(restored.content, "original content");
    assert_eq!(restored.status, cas_types::RuleStatus::Draft);
}

/// T5 cas-07d7: Skill.share must round-trip. First code-review round
/// found the skill SELECT/INSERT/UPDATE had silently skipped the
/// `share` column; this test exists to catch that regression class.
#[test]
fn test_skill_share_roundtrip() {
    let temp = TempDir::new().unwrap();
    let store = SqliteSkillStore::open(temp.path()).unwrap();
    store.init().unwrap();

    let mut s = Skill::new("sk-share-01".to_string(), "test".to_string());
    s.share = Some(ShareScope::Team);
    store.add(&s).unwrap();
    assert_eq!(
        store.get("sk-share-01").unwrap().share,
        Some(ShareScope::Team)
    );

    let mut loaded = store.get("sk-share-01").unwrap();
    loaded.share = Some(ShareScope::Private);
    store.update(&loaded).unwrap();
    assert_eq!(
        store.get("sk-share-01").unwrap().share,
        Some(ShareScope::Private)
    );
}

/// T5 cas-07d7: Task.share must round-trip through the many SELECT
/// sites and the INSERT/UPDATE paths.
#[test]
fn test_task_share_roundtrip() {
    let temp = TempDir::new().unwrap();
    let store = SqliteTaskStore::open(temp.path()).unwrap();
    store.init().unwrap();

    let mut t = Task::new("cas-share".to_string(), "test task".to_string());
    t.share = Some(ShareScope::Team);
    store.add(&t).unwrap();
    assert_eq!(
        store.get("cas-share").unwrap().share,
        Some(ShareScope::Team)
    );

    let mut loaded = store.get("cas-share").unwrap();
    loaded.share = Some(ShareScope::Private);
    store.update(&loaded).unwrap();
    assert_eq!(
        store.get("cas-share").unwrap().share,
        Some(ShareScope::Private)
    );
}

/// Helper to add session signal columns for testing
/// These columns are normally added by migrations m042-m044
fn add_session_signal_columns(store: &SqliteStore) {
    let conn = store.conn.lock().unwrap();
    // Ignore errors if columns already exist
    let _ = conn.execute("ALTER TABLE sessions ADD COLUMN outcome TEXT", []);
    let _ = conn.execute("ALTER TABLE sessions ADD COLUMN friction_score REAL", []);
    let _ = conn.execute(
        "ALTER TABLE sessions ADD COLUMN delight_count INTEGER DEFAULT 0",
        [],
    );
}

#[test]
fn test_signals_outcome_summary() {
    let temp = TempDir::new().unwrap();
    let store = SqliteStore::open(temp.path()).unwrap();
    store.init().unwrap();
    add_session_signal_columns(&store);

    // Create sessions with different outcomes
    let session1 = cas_types::Session::new(
        "session-1".to_string(),
        "/test".to_string(),
        Some("default".to_string()),
    );
    store.start_session(&session1).unwrap();
    store.end_session("session-1").unwrap();
    store
        .update_session_outcome("session-1", cas_types::SessionOutcome::TasksCompleted)
        .unwrap();

    let session2 = cas_types::Session::new(
        "session-2".to_string(),
        "/test".to_string(),
        Some("default".to_string()),
    );
    store.start_session(&session2).unwrap();
    store.end_session("session-2").unwrap();
    store
        .update_session_outcome("session-2", cas_types::SessionOutcome::TasksCompleted)
        .unwrap();

    let session3 = cas_types::Session::new(
        "session-3".to_string(),
        "/test".to_string(),
        Some("default".to_string()),
    );
    store.start_session(&session3).unwrap();
    store.end_session("session-3").unwrap();
    store
        .update_session_outcome("session-3", cas_types::SessionOutcome::Abandoned)
        .unwrap();

    // Query outcome summary
    let results = store.outcome_summary(30).unwrap();
    assert_eq!(results.len(), 2);

    // Find tasks_completed
    let completed = results
        .iter()
        .find(|(o, _, _)| o == "tasks_completed")
        .unwrap();
    assert_eq!(completed.1, 2); // count
    assert!((completed.2 - 66.67).abs() < 0.1); // ~66.67%

    // Find abandoned
    let abandoned = results.iter().find(|(o, _, _)| o == "abandoned").unwrap();
    assert_eq!(abandoned.1, 1); // count
    assert!((abandoned.2 - 33.33).abs() < 0.1); // ~33.33%
}
