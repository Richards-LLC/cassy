use crate::task_store::*;
use tempfile::TempDir;

fn create_test_store() -> (TempDir, SqliteTaskStore) {
    let temp = TempDir::new().unwrap();
    let store = SqliteTaskStore::open(temp.path()).unwrap();
    store.init().unwrap();
    (temp, store)
}

#[test]
fn test_task_crud() {
    let (_temp, store) = create_test_store();

    // Create task
    let id = store.generate_id().unwrap();
    let mut task = Task::new(id.clone(), "Test task".to_string());
    task.priority = Priority::HIGH;
    store.add(&task).unwrap();

    // Get task
    let retrieved = store.get(&id).unwrap();
    assert_eq!(retrieved.title, "Test task");
    assert_eq!(retrieved.priority, Priority::HIGH);

    // Update task
    task.status = TaskStatus::InProgress;
    task.notes = "Working on it".to_string();
    store.update(&task).unwrap();

    let retrieved = store.get(&id).unwrap();
    assert_eq!(retrieved.status, TaskStatus::InProgress);
    assert_eq!(retrieved.notes, "Working on it");

    // List tasks
    let all_tasks = store.list(None).unwrap();
    assert_eq!(all_tasks.len(), 1);

    let in_progress = store.list(Some(TaskStatus::InProgress)).unwrap();
    assert_eq!(in_progress.len(), 1);

    let open = store.list(Some(TaskStatus::Open)).unwrap();
    assert_eq!(open.len(), 0);

    // Delete task
    store.delete(&id).unwrap();
    assert!(store.get(&id).is_err());
}

#[test]
fn create_atomic_records_creator_session_on_task_created_event() {
    let (_temp, store) = create_test_store();
    {
        let conn = store.conn.lock().unwrap();
        conn.execute_batch(crate::EVENT_SCHEMA).unwrap();
    }
    let task = Task::new(
        store.generate_id().unwrap(),
        "Session-authored task".to_string(),
    );

    store
        .create_atomic(&task, &[], None, Some("outer-session"))
        .unwrap();

    let conn = store.conn.lock().unwrap();
    let session_id: Option<String> = conn
        .query_row(
            "select session_id from events where entity_id = ?1 and event_type = 'task_created'",
            [&task.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(session_id.as_deref(), Some("outer-session"));
}

#[test]
fn test_task_depth_roundtrip() {
    use cas_types::TaskDepth;
    let (_temp, store) = create_test_store();

    // Default (no depth specified) reads back as Deep.
    let id = store.generate_id().unwrap();
    let task = Task::new(id.clone(), "deep by default".to_string());
    store.add(&task).unwrap();
    assert_eq!(store.get(&id).unwrap().depth, TaskDepth::Deep);

    // Create with Light, read back Light.
    let light_id = store.generate_id().unwrap();
    let mut light = Task::new(light_id.clone(), "light task".to_string());
    light.depth = TaskDepth::Light;
    store.add(&light).unwrap();
    assert_eq!(store.get(&light_id).unwrap().depth, TaskDepth::Light);

    // Update Light -> Deep; round-trips through the store.
    let mut fetched = store.get(&light_id).unwrap();
    fetched.depth = TaskDepth::Deep;
    store.update(&fetched).unwrap();
    assert_eq!(store.get(&light_id).unwrap().depth, TaskDepth::Deep);
}

#[test]
fn closed_to_non_closed_update_clears_close_cycle_authority() {
    let (_temp, store) = create_test_store();
    let mut task = Task::new(
        store.generate_id().unwrap(),
        "Task with close-cycle evidence".to_string(),
    );
    task.status = TaskStatus::Closed;
    task.deliverables.factory_branch_anchor = Some("old-close-sha".to_string());
    task.deliverables.parked_branch = Some("factory/worker".to_string());
    task.deliverables.work_target = Some(cas_types::WorkTarget {
        repo_selector: "remote:github.com/org/repo".to_string(),
        target_branch: "main".to_string(),
    });
    task.deliverables.pre_close_hook = Some(cas_types::PreCloseHookEvidence {
        repo_selector: "remote:github.com/org/repo".to_string(),
        target_branch: "main".to_string(),
        worktree_branch: Some("factory/worker".to_string()),
        task_tip: Some("old-close-sha".to_string()),
    });
    task.deliverables.negative_result = Some(cas_types::NegativeResultEvidence {
        artifact_path: "/durable/task/proof.json".to_string(),
        reference: "https://github.com/pippenz/cas/pull/242".to_string(),
        rationale: "experiment regressed".to_string(),
        supervisor_id: "supervisor-1".to_string(),
        supervisor_name: "supervisor".to_string(),
    });
    store.add(&task).unwrap();

    task.status = TaskStatus::Blocked;
    store.update(&task).unwrap();

    let reopened = store.get(&task.id).unwrap();
    assert_eq!(reopened.status, TaskStatus::Blocked);
    assert!(
        reopened.deliverables.factory_branch_anchor.is_none(),
        "the prior close cycle's commit receipt must be invalidated"
    );
    assert!(
        reopened.deliverables.negative_result.is_none(),
        "reopening must not let a prior negative-result decision exempt fresh work from delivery gates"
    );
    assert_eq!(
        reopened.deliverables.parked_branch.as_deref(),
        Some("factory/worker"),
        "branch identity remains useful for diagnostics and matches cas_task_reopen"
    );
    assert_eq!(
        reopened
            .deliverables
            .work_target
            .as_ref()
            .map(|target| target.repo_selector.as_str()),
        Some("remote:github.com/org/repo"),
        "close-cycle updates must preserve the durable work target"
    );
    assert_eq!(
        reopened
            .deliverables
            .pre_close_hook
            .as_ref()
            .and_then(|evidence| evidence.task_tip.as_deref()),
        Some("old-close-sha"),
        "task updates must not overwrite portable hook audit evidence"
    );
}

#[test]
fn awaiting_merge_conflict_rework_clears_all_parked_merge_state() {
    let (_temp, store) = create_test_store();
    let mut task = Task::new(
        store.generate_id().unwrap(),
        "Conflicted parked task".to_string(),
    );
    task.status = TaskStatus::AwaitingMerge;
    task.deliverables.factory_branch_anchor = Some("conflicted-sha".to_string());
    task.deliverables.parked_branch = Some("factory/worker".to_string());
    task.deliverables.merge_conflicted = true;
    store.add(&task).unwrap();

    task.status = TaskStatus::InProgress;
    store.update(&task).unwrap();

    let resumed = store.get(&task.id).unwrap();
    assert_eq!(resumed.status, TaskStatus::InProgress);
    assert!(resumed.deliverables.factory_branch_anchor.is_none());
    assert!(resumed.deliverables.parked_branch.is_none());
    assert!(!resumed.deliverables.merge_conflicted);
}

#[test]
fn test_dependencies() {
    let (_temp, store) = create_test_store();

    // Create two tasks
    let task1 = Task::new(store.generate_id().unwrap(), "Task 1".to_string());
    let task2 = Task::new(store.generate_id().unwrap(), "Task 2".to_string());
    store.add(&task1).unwrap();
    store.add(&task2).unwrap();

    // Add dependency: task2 blocks task1
    let dep = Dependency::new(task1.id.clone(), task2.id.clone(), DependencyType::Blocks);
    store.add_dependency(&dep).unwrap();

    // Check dependencies
    let deps = store.get_dependencies(&task1.id).unwrap();
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].to_id, task2.id);

    // Check dependents
    let dependents = store.get_dependents(&task2.id).unwrap();
    assert_eq!(dependents.len(), 1);
    assert_eq!(dependents[0].from_id, task1.id);

    // Check blockers
    let blockers = store.get_blockers(&task1.id).unwrap();
    assert_eq!(blockers.len(), 1);
    assert_eq!(blockers[0].id, task2.id);

    // Remove dependency
    store.remove_dependency(&task1.id, &task2.id).unwrap();
    let deps = store.get_dependencies(&task1.id).unwrap();
    assert!(deps.is_empty());
}

#[test]
fn test_ready_tasks() {
    let (_temp, store) = create_test_store();

    // Create tasks
    let task1 = Task::new(store.generate_id().unwrap(), "Task 1".to_string());
    let task2 = Task::new(store.generate_id().unwrap(), "Task 2".to_string());
    let task3 = Task::new(store.generate_id().unwrap(), "Task 3".to_string());
    store.add(&task1).unwrap();
    store.add(&task2).unwrap();
    store.add(&task3).unwrap();

    // All should be ready initially
    let ready = store.list_ready().unwrap();
    assert_eq!(ready.len(), 3);

    // Add blocking dependency: task2 blocks task1
    let dep = Dependency::new(task1.id.clone(), task2.id.clone(), DependencyType::Blocks);
    store.add_dependency(&dep).unwrap();

    // task1 should not be ready
    let ready = store.list_ready().unwrap();
    assert_eq!(ready.len(), 2);
    assert!(!ready.iter().any(|t| t.id == task1.id));

    // Close task2, task1 should be ready again
    let mut task2_updated = store.get(&task2.id).unwrap();
    task2_updated.status = TaskStatus::Closed;
    store.update(&task2_updated).unwrap();

    let ready = store.list_ready().unwrap();
    assert_eq!(ready.len(), 2); // task1 and task3 (task2 is closed)
    assert!(ready.iter().any(|t| t.id == task1.id));
}

#[test]
fn test_cycle_detection() {
    let (_temp, store) = create_test_store();

    // Create tasks
    let task1 = Task::new(store.generate_id().unwrap(), "Task 1".to_string());
    let task2 = Task::new(store.generate_id().unwrap(), "Task 2".to_string());
    let task3 = Task::new(store.generate_id().unwrap(), "Task 3".to_string());
    store.add(&task1).unwrap();
    store.add(&task2).unwrap();
    store.add(&task3).unwrap();

    // Create chain: task1 -> task2 -> task3
    let dep1 = Dependency::new(task1.id.clone(), task2.id.clone(), DependencyType::Blocks);
    let dep2 = Dependency::new(task2.id.clone(), task3.id.clone(), DependencyType::Blocks);
    store.add_dependency(&dep1).unwrap();
    store.add_dependency(&dep2).unwrap();

    // Trying to add task3 -> task1 should detect cycle
    assert!(store.would_create_cycle(&task3.id, &task1.id).unwrap());

    // But task3 -> task2 won't create a cycle (already exists in reverse)
    assert!(!store.would_create_cycle(&task1.id, &task3.id).unwrap());
}

#[test]
fn test_sibling_notes_and_parent_epic() {
    let (_temp, store) = create_test_store();

    // Create epic
    let mut epic = Task::new(store.generate_id().unwrap(), "Test Epic".to_string());
    epic.task_type = TaskType::Epic;
    store.add(&epic).unwrap();

    // Create subtasks with notes
    let mut subtask1 = Task::new(store.generate_id().unwrap(), "Subtask 1".to_string());
    subtask1.notes = "[2026-02-03 14:30] 💡 DISCOVERY API uses camelCase".to_string();
    store.add(&subtask1).unwrap();

    let mut subtask2 = Task::new(store.generate_id().unwrap(), "Subtask 2".to_string());
    subtask2.notes = "[2026-02-03 15:00] ✅ DECISION Use existing helper".to_string();
    store.add(&subtask2).unwrap();

    let subtask3 = Task::new(store.generate_id().unwrap(), "Subtask 3".to_string());
    // No notes on subtask3
    store.add(&subtask3).unwrap();

    // Link subtasks to epic via ParentChild dependency
    let dep1 = Dependency::new(
        subtask1.id.clone(),
        epic.id.clone(),
        DependencyType::ParentChild,
    );
    let dep2 = Dependency::new(
        subtask2.id.clone(),
        epic.id.clone(),
        DependencyType::ParentChild,
    );
    let dep3 = Dependency::new(
        subtask3.id.clone(),
        epic.id.clone(),
        DependencyType::ParentChild,
    );
    store.add_dependency(&dep1).unwrap();
    store.add_dependency(&dep2).unwrap();
    store.add_dependency(&dep3).unwrap();

    // Test get_sibling_notes from subtask3's perspective
    let siblings = store.get_sibling_notes(&epic.id, &subtask3.id).unwrap();
    assert_eq!(siblings.len(), 2); // subtask1 and subtask2 have notes

    // Verify the notes content
    let notes_content: Vec<&str> = siblings.iter().map(|(_, _, n)| n.as_str()).collect();
    assert!(notes_content.iter().any(|n| n.contains("camelCase")));
    assert!(notes_content.iter().any(|n| n.contains("existing helper")));

    // Test get_parent_epic
    let parent = store.get_parent_epic(&subtask1.id).unwrap();
    assert!(parent.is_some());
    assert_eq!(parent.unwrap().id, epic.id);

    // Epic itself has no parent
    let no_parent = store.get_parent_epic(&epic.id).unwrap();
    assert!(no_parent.is_none());
}

#[test]
fn test_delete_rolls_back_on_missing_task() {
    let (_temp, store) = create_test_store();

    // Create a task and add a dependency to it
    let task1 = Task::new(store.generate_id().unwrap(), "Task 1".to_string());
    let task2 = Task::new(store.generate_id().unwrap(), "Task 2".to_string());
    store.add(&task1).unwrap();
    store.add(&task2).unwrap();

    let dep = Dependency::new(task1.id.clone(), task2.id.clone(), DependencyType::Blocks);
    store.add_dependency(&dep).unwrap();

    // Delete task1 — should atomically remove task + dependencies
    store.delete(&task1.id).unwrap();

    // Task should be gone
    assert!(store.get(&task1.id).is_err());

    // Dependencies referencing task1 should also be gone
    let deps = store.get_dependents(&task2.id).unwrap();
    assert!(
        deps.is_empty(),
        "Dependencies should be cleaned up atomically with task delete"
    );

    // Deleting non-existent task should error (and not corrupt anything)
    let result = store.delete("non-existent");
    assert!(result.is_err());

    // task2 should still be intact
    let task2_check = store.get(&task2.id).unwrap();
    assert_eq!(task2_check.title, "Task 2");
}

/// cas-ec74: `updated_at` is store-owned on the `update()` path, and the stamp
/// the store actually wrote is returned to the caller.
///
/// Before this, `update()` returned `Result<()>` and re-read the clock inside
/// the UPDATE, so the persisted stamp was invisible. Callers that needed it —
/// every lifecycle-notification producer — took a second `Utc::now()` and
/// derived an occurrence from it, which could never match the stored row. The
/// producer side had no test at all; the existing CRUD tests just `unwrap()`
/// the result and never look at the timestamp.
#[test]
fn update_owns_updated_at_and_returns_the_stamp_it_persisted() {
    let (_temp, store) = create_test_store();

    let id = store.generate_id().unwrap();
    let mut task = Task::new(id.clone(), "Store-owned timestamp".to_string());
    store.add(&task).unwrap();

    // A caller-supplied sentinel, far enough in the past that it cannot be
    // confused with a clock read taken during this test.
    let sentinel = chrono::Utc::now() - chrono::Duration::days(365);
    task.updated_at = sentinel;
    task.status = TaskStatus::InProgress;

    let returned = store.update(&task).unwrap();
    let persisted = store.get(&id).unwrap().updated_at;

    assert_ne!(
        persisted, sentinel,
        "updated_at is store-owned: a caller cannot dictate it through update()"
    );
    assert_eq!(
        persisted, returned,
        "the returned stamp must BE the persisted stamp — if these can differ, \
         callers are back to guessing with a second clock read, which is the \
         whole defect this contract removes"
    );
}

/// cas-ec74: the returned stamp is authoritative across repeated writes, not
/// merely "close enough". Two updates must return two distinct stamps, each
/// matching what the row carried at that moment.
#[test]
fn each_update_returns_its_own_persisted_stamp() {
    let (_temp, store) = create_test_store();

    let id = store.generate_id().unwrap();
    let mut task = Task::new(id.clone(), "Sequential writes".to_string());
    store.add(&task).unwrap();

    task.notes = "first".to_string();
    let first = store.update(&task).unwrap();
    assert_eq!(store.get(&id).unwrap().updated_at, first);

    task.notes = "second".to_string();
    let second = store.update(&task).unwrap();
    assert_eq!(store.get(&id).unwrap().updated_at, second);

    assert!(
        second >= first,
        "the store clock must not run backwards between writes"
    );
}

/// cas-ec74: `reopen_exact_with_conn` is the ONE sanctioned exception to the
/// store-owned rule — there `updated_at` is a compare-and-swap key, not a
/// value the store gets to choose. Driven here through its public entry point
/// `reopen_closed_task_atomic`. Pins that the lock still rejects a mismatched
/// expectation, so documenting the exception cannot quietly become removing it.
#[test]
fn reopen_exact_rejects_a_mismatched_expected_updated_at() {
    let temp = TempDir::new().unwrap();
    let store = SqliteTaskStore::open(temp.path()).unwrap();
    store.init().unwrap();

    let id = store.generate_id().unwrap();
    let mut task = Task::new(id.clone(), "Optimistic lock".to_string());
    store.add(&task).unwrap();
    task.status = TaskStatus::Closed;
    task.closed_at = Some(chrono::Utc::now());
    let closed_at = store.update(&task).unwrap();

    let mut reopened = task.clone();
    reopened.status = TaskStatus::Open;
    reopened.closed_at = None;
    reopened.updated_at = chrono::Utc::now();

    // Wrong expectation: another write landed between read and reopen.
    let stale = crate::reopen_closed_task_atomic(
        temp.path(),
        &reopened,
        closed_at - chrono::Duration::seconds(1),
        crate::ParentDependencyUpdate::Unchanged,
        None,
    );
    assert!(
        stale.is_err(),
        "a mismatched expected_updated_at must be refused, not silently applied"
    );
    assert_eq!(
        store.get(&id).unwrap().status,
        TaskStatus::Closed,
        "the refused reopen must leave the row untouched"
    );

    // Correct expectation: the CAS succeeds and writes the caller's value
    // through verbatim — the documented exception to store-owned updated_at.
    crate::reopen_closed_task_atomic(
        temp.path(),
        &reopened,
        closed_at,
        crate::ParentDependencyUpdate::Unchanged,
        None,
    )
    .expect("matching expected_updated_at should succeed");

    let after = store.get(&id).unwrap();
    assert_eq!(after.status, TaskStatus::Open);
    assert_eq!(
        after.updated_at, reopened.updated_at,
        "reopen_closed_task_atomic persists the caller's updated_at verbatim; \
         it is an optimistic-concurrency key, not a general write path"
    );
}
