//! Explicit recovery of legacy personal transport in team-only projects.
use super::SyncQueue;
use crate::error::CasError;
use serde::Serialize;

/// Identifiers and decisions only: never contains queued content.
#[derive(Debug, Serialize)]
pub struct TeamBackfillRow {
    pub id: i64,
    pub entity_type: super::EntityType,
    pub entity_id: String,
    pub eligible: bool,
    pub reason: &'static str,
}

impl SyncQueue {
    /// Preview by default; applying adds team transport without retiring personal rows.
    pub fn migrate_personal_to_team(
        &self,
        _team: &str,
        _project: &str,
        _apply: bool,
    ) -> Result<Vec<TeamBackfillRow>, CasError> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::{EntityType, SyncOperation};
    use crate::store::TaskStore;

    #[test]
    fn cas_db92_backfill_preserves_scope_and_retires_only_itemized_acknowledged_snapshots() {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        let queue = SyncQueue::open(&root).unwrap();
        queue.init().unwrap();
        let tasks = crate::store::open_task_store_local(&root).unwrap();
        for (id, origin, share) in [
            ("owned", "p", None),
            ("newer", "p", None),
            ("foreign", "other", None),
            ("private", "p", Some("private")),
            ("unknown", "unknown", None),
            ("global", "p", None),
        ] {
            let mut task = crate::types::Task::new(id.into(), id.into());
            task.origin_project = Some(origin.into());
            tasks.add(&task).unwrap();
            let payload = serde_json::json!({"id":id, "origin_project":origin,
                "scope":if id == "global" {"global"} else {"project"}, "share":share});
            queue
                .enqueue(
                    EntityType::Task,
                    id,
                    SyncOperation::Upsert,
                    Some(&payload.to_string()),
                )
                .unwrap();
        }
        // A stale public payload cannot override a newer private local policy.
        queue
            .conn
            .lock()
            .unwrap()
            .execute("UPDATE tasks SET share='private' WHERE id='private'", [])
            .unwrap();
        queue
            .enqueue(EntityType::Task, "deleted", SyncOperation::Delete, None)
            .unwrap();
        queue
            .enqueue_for_team(EntityType::Task, "newer", SyncOperation::Delete, None, "t")
            .unwrap();
        queue.conn.lock().unwrap().execute("INSERT INTO dependencies(from_id,to_id,dep_type,created_at) VALUES('owned','newer','blocks',?1)", [chrono::Utc::now().to_rfc3339()]).unwrap();
        for (id, from, to) in [
            ("edge", "owned", "newer"),
            ("private-edge", "owned", "private"),
        ] {
            let payload = serde_json::json!({"from_id":from,"to_id":to,"dep_type":"blocks","origin_project":"p"});
            queue
                .enqueue(
                    EntityType::TaskDependency,
                    id,
                    SyncOperation::Upsert,
                    Some(&payload.to_string()),
                )
                .unwrap();
        }
        let before = queue.list_all(100).unwrap();
        let readonly = SyncQueue::open_read_only(&root).unwrap();
        let preview = readonly.migrate_personal_to_team("t", "p", false).unwrap();
        assert_eq!(
            preview
                .iter()
                .filter(|r| r.eligible)
                .map(|r| r.entity_id.as_str())
                .collect::<Vec<_>>(),
            vec!["owned", "edge"]
        );
        assert_eq!(
            serde_json::to_value(queue.list_all(100).unwrap()).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
        let applied = queue.migrate_personal_to_team("t", "p", true).unwrap();
        assert_eq!(applied.iter().filter(|r| r.eligible).count(), 2);
        assert_eq!(
            queue.personal_row_count().unwrap(),
            9,
            "enqueue is not delivery"
        );
        let team = queue.pending_for_team("t", 100, 5).unwrap();
        assert_eq!(team.len(), 3);
        assert!(
            team.iter()
                .any(|r| r.entity_id == "newer" && r.operation == SyncOperation::Delete)
        );
        assert!(
            !queue
                .migrate_personal_to_team("t", "p", true)
                .unwrap()
                .iter()
                .any(|r| r.eligible),
            "idempotent and no overwrites"
        );
        assert_eq!(queue.neutralize_team_only_personal("t", "p").unwrap(), 0);
        let sent = team.iter().find(|r| r.entity_id == "owned").unwrap();
        let personal = queue.personal_delivery_snapshots().unwrap();
        queue
            .acknowledge_team_write(
                sent,
                &serde_json::from_str(sent.payload.as_deref().unwrap()).unwrap(),
                "t",
                "p",
                &personal,
            )
            .unwrap();
        drop(queue);
        let reopened = SyncQueue::open(&root).unwrap();
        assert_eq!(reopened.neutralize_team_only_personal("t", "p").unwrap(), 1);
        assert_eq!(reopened.personal_row_count().unwrap(), 8);
    }
}
