//! A hold hides action invitations without consuming their delivery receipts.
use crate::{PromptQueueStore, SqlitePromptQueueStore, SqliteTaskStore, TaskStore};

#[test]
fn project_hold_filters_action_lists_and_unread_relays_without_receipting_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let tasks = SqliteTaskStore::open(root).unwrap();
    tasks.init().unwrap();
    let mut task = cas_types::Task::new("cas-held".into(), "held delivery".into());
    tasks.add(&task).unwrap();
    let queue = SqlitePromptQueueStore::open(root).unwrap();
    queue.init().unwrap();
    let pending = format!("<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-held\" new=\"awaiting_merge\" occurrence=\"{}\">merge now</task-lifecycle>", chrono::Utc::now().to_rfc3339());
    let stalled = "<worker-attention kind=\"supervisor_stalled\" worker=\"supervisor\" notification_id=\"42\">dispatch now</worker-attention>";
    let ids = [pending.as_str(), stalled].map(|body| queue.enqueue("director", "supervisor", body).unwrap());
    let normal = queue.enqueue("operator", "supervisor", "Read the plan").unwrap();
    std::fs::create_dir_all(root.join("operator")).unwrap();
    let path = root.join("operator/project-hold.json");
    std::fs::write(&path, r#"{"active":{"reason":"Review the plan","operator":"fixture","set_at":"2026-11-03T14:00:00Z"},"history":[]}"#).unwrap();
    assert!(tasks.list_ready().unwrap().is_empty());
    task.status = cas_types::TaskStatus::AwaitingMerge;
    tasks.update(&task).unwrap();
    assert!(tasks.list(Some(cas_types::TaskStatus::AwaitingMerge)).unwrap().is_empty());
    assert!(tasks.list(None).unwrap().is_empty());
    assert_eq!(tasks.get(&task.id).unwrap().status, cas_types::TaskStatus::AwaitingMerge);
    let surfaced = queue.surface_unseen_for_recipient("supervisor", None, 10).unwrap();
    assert_eq!(surfaced.iter().map(|p| p.id).collect::<Vec<_>>(), vec![normal]);
    for id in ids { assert!(queue.recipient_receipt(id, "supervisor").unwrap().is_none()); }
    std::fs::remove_file(path).unwrap();
    assert_eq!(tasks.list(Some(cas_types::TaskStatus::AwaitingMerge)).unwrap().len(), 1);
    let surfaced = queue.surface_unseen_for_recipient("supervisor", None, 10).unwrap();
    assert_eq!(surfaced.iter().map(|p| p.id).collect::<Vec<_>>(), ids);
    task.status = cas_types::TaskStatus::Open;
    tasks.update(&task).unwrap();
    assert_eq!(tasks.list_ready().unwrap().len(), 1);
}
