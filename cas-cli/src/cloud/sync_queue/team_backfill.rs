//! Explicit recovery of legacy personal transport in team-only projects.
use super::SyncQueue;
use crate::error::CasError;
use serde::Serialize;
use std::collections::BTreeSet;

/// Identifiers and decisions only: never contains queued content.
#[derive(Debug, Serialize)]
pub struct TeamBackfillRow {
    pub id: i64,
    pub entity_type: super::EntityType,
    pub entity_id: String,
    pub eligible: bool,
    pub reason: &'static str,
}

// Both the snapshot and the current local sharing policy must authorize a copy.
// Exact origin/payload equality keeps the existing itemized ACK contract intact.
fn local_policy(
    conn: &rusqlite::Connection,
    kind: super::EntityType,
    id: &str,
    team: &str,
    project: &str,
) -> bool {
    use super::EntityType;
    use rusqlite::OptionalExtension;
    let (table, scope, preference) = match kind {
        EntityType::Task => ("tasks", "'project'", "0"),
        EntityType::Entry => ("entries", "scope", "type = 'preference'"),
        EntityType::Rule => ("rules", "scope", "0"),
        _ => return false,
    };
    conn.query_row(
        &format!(
            "SELECT origin_project = ?2 AND {scope} = 'project'
        AND (share IS NULL OR share = 'team') AND NOT ({preference} AND share IS NULL)
        AND (coalesce(team_id,'') = '' OR team_id = ?3) FROM {table} WHERE id = ?1"
        ),
        rusqlite::params![id, project, team],
        |row| row.get::<_, Option<bool>>(0),
    )
    .optional()
    .ok()
    .flatten()
    .flatten()
    .unwrap_or(false)
}

fn eligibility(
    conn: &rusqlite::Connection,
    row: &super::QueuedSync,
    team: &str,
    project: &str,
) -> &'static str {
    use super::{EntityType, SyncOperation};
    if row.operation != SyncOperation::Upsert {
        return "delete_preserved";
    }
    if row
        .project_id
        .as_deref()
        .is_some_and(|route| route != project)
    {
        return "foreign_route_preserved";
    }
    if !matches!(
        row.entity_type,
        EntityType::Task | EntityType::Entry | EntityType::Rule | EntityType::TaskDependency
    ) {
        return "unsupported_entity_preserved";
    }
    let Some(value) = row
        .payload
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
    else {
        return "invalid_payload";
    };
    if value.get("origin_project").and_then(|v| v.as_str()) != Some(project) {
        return "foreign_or_unknown_origin";
    }
    if value
        .get("team_id")
        .and_then(|v| v.as_str())
        .is_some_and(|id| !id.is_empty() && id != team)
    {
        return "other_team_preserved";
    }
    if row.entity_type == EntityType::TaskDependency {
        let (Some(from), Some(to), Some(kind)) = (
            value.get("from_id").and_then(|v| v.as_str()),
            value.get("to_id").and_then(|v| v.as_str()),
            value.get("dep_type").and_then(|v| v.as_str()),
        ) else {
            return "invalid_dependency";
        };
        if !local_policy(conn, EntityType::Task, from, team, project)
            || !local_policy(conn, EntityType::Task, to, team, project)
        {
            return "unshared_or_foreign_endpoint";
        }
        let exists = conn.query_row("SELECT EXISTS(SELECT 1 FROM dependencies WHERE from_id=?1 AND to_id=?2 AND dep_type=?3)", rusqlite::params![from,to,kind], |r|r.get::<_,bool>(0)).unwrap_or(false);
        if !exists {
            return "missing_local_dependency";
        }
    } else {
        if value.get("id").and_then(|v| v.as_str()) != Some(&row.entity_id) {
            return "payload_id_mismatch";
        }
        if value.get("scope").and_then(|v| v.as_str()) != Some("project")
            || value
                .get("share")
                .and_then(|v| v.as_str())
                .is_some_and(|v| v != "team")
            || (row.entity_type == EntityType::Entry
                && value.get("entry_type").and_then(|v| v.as_str()) == Some("preference")
                && value.get("share").and_then(|v| v.as_str()) != Some("team"))
            || !local_policy(conn, row.entity_type, &row.entity_id, team, project)
        {
            return "unshared_or_foreign_local_row";
        }
    }
    let queued: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM sync_queue WHERE entity_type=?1 AND entity_id=?2 AND team_id=?3)", rusqlite::params![row.entity_type.as_str(),row.entity_id,team],|r|r.get(0)).unwrap_or(true);
    if queued {
        "existing_team_intent_preserved"
    } else {
        "enqueue_team_snapshot"
    }
}

impl SyncQueue {
    /// Preview by default; applying adds team transport without retiring personal rows.
    /// Policy, snapshots and existing team intents are read in one transaction.
    /// No remote I/O, source-entity rewrite or delivery receipt is fabricated.
    pub fn migrate_personal_to_team(
        &self,
        team: &str,
        project: &str,
        apply: bool,
    ) -> Result<Vec<TeamBackfillRow>, CasError> {
        if team.trim().is_empty() || project.trim().is_empty() {
            return Err(CasError::Other(
                "An active team and canonical project are required".into(),
            ));
        }
        let mut conn = self.conn.lock().unwrap();
        let tx = conn.transaction_with_behavior(if apply {
            rusqlite::TransactionBehavior::Immediate
        } else {
            rusqlite::TransactionBehavior::Deferred
        })?;
        let rows = {
            let mut stmt = tx.prepare("SELECT id,entity_type,entity_id,operation,payload,team_id,project_id,created_at,retry_count,last_error,last_outcome,last_reason,failed_client_version FROM sync_queue WHERE team_id='' ORDER BY id")?;
            stmt.query_map([], Self::map_row)?
                .collect::<Result<Vec<_>, _>>()?
        };
        let mut decisions = Vec::with_capacity(rows.len());
        let mut planned = BTreeSet::new();
        for row in &rows {
            let mut reason = eligibility(&tx, row, team, project);
            if reason == "enqueue_team_snapshot"
                && !planned.insert((row.entity_type.as_str().to_string(), row.entity_id.clone()))
            {
                reason = "duplicate_personal_preserved";
            }
            let eligible = reason == "enqueue_team_snapshot";
            decisions.push(TeamBackfillRow {
                id: row.id,
                entity_type: row.entity_type,
                entity_id: row.entity_id.clone(),
                eligible,
                reason,
            });
        }
        if apply {
            for (row, decision) in rows.iter().zip(&decisions) {
                if decision.eligible {
                    tx.execute("INSERT INTO sync_queue(entity_type,entity_id,operation,payload,team_id,project_id,created_at,retry_count) VALUES(?1,?2,'upsert',?3,?4,?5,?6,0)",rusqlite::params![row.entity_type.as_str(),row.entity_id,row.payload,team,row.project_id,chrono::Utc::now().to_rfc3339()])?;
                }
            }
        }
        tx.commit()?;
        Ok(decisions)
    }

    /// Count only personal rows that would produce a new team snapshot.
    /// Uses the same read-only eligibility plan as the explicit migration.
    pub(crate) fn migratable_personal_row_count(
        &self,
        team: &str,
        project: &str,
    ) -> Result<usize, CasError> {
        Ok(self
            .migrate_personal_to_team(team, project, false)?
            .iter()
            .filter(|row| row.eligible)
            .count())
    }

    pub(crate) fn personal_backlog_remedy(team_only: bool) -> &'static str {
        if team_only {
            "Review `cas cloud queue migrate-to-team`; apply with `cas cloud queue migrate-to-team --apply`, then run `cas cloud sync`."
        } else {
            "If this project is team-only, run `cas config set cloud.team_only true`, review `cas cloud queue migrate-to-team`, then apply with `cas cloud queue migrate-to-team --apply` and run `cas cloud sync`."
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cloud::{EntityType, SyncOperation};

    #[test]
    fn cas_5aeb_backfill_preview_and_apply_agree_for_duplicate_personal_rows() {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        crate::cloud::set_canonical_id_in_config_toml(&root, "p").unwrap();
        let queue = SyncQueue::open(&root).unwrap();
        queue.init().unwrap();
        let mut task = crate::types::Task::new("duplicate".into(), "duplicate".into());
        task.origin_project = Some("p".into());
        crate::store::open_task_store_local(&root).unwrap().add(&task).unwrap();
        queue.enqueue(
            EntityType::Task,
            "duplicate",
            SyncOperation::Upsert,
            Some(&serde_json::to_string(&task).unwrap()),
        ).unwrap();
        let personal_id = queue.list_all(10).unwrap()[0].id;
        {
            let conn = queue.conn.lock().unwrap();
            conn.execute_batch("DROP INDEX idx_sync_queue_entity_team_project;").unwrap();
            conn.execute(
                "INSERT INTO sync_queue(entity_type,entity_id,operation,payload,team_id,project_id,created_at,retry_count)
                 SELECT entity_type,entity_id,operation,payload,team_id,project_id,created_at,retry_count FROM sync_queue WHERE id=?1",
                [personal_id],
            ).unwrap();
        }

        let preview = SyncQueue::open_read_only(&root).unwrap()
            .migrate_personal_to_team("t", "p", false).unwrap();
        let applied = queue.migrate_personal_to_team("t", "p", true).unwrap();
        let count = |rows: &[TeamBackfillRow]| rows.iter().filter(|row| row.eligible).count();
        assert_eq!(count(&preview), 1);
        assert_eq!(count(&applied), count(&preview));
        assert_eq!(queue.pending_for_team("t", 10, 5).unwrap().len(), 1);
        assert_eq!(queue.personal_row_count().unwrap(), 2, "both legacy rows remain until delivery");
    }

    #[test]
    fn cas_db92_backfill_preserves_scope_and_retires_only_itemized_acknowledged_snapshots() {
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        crate::cloud::set_canonical_id_in_config_toml(&root, "p").unwrap();
        let queue = SyncQueue::open(&root).unwrap();
        queue.init().unwrap();
        let tasks = crate::store::open_task_store_local(&root).unwrap();
        for (id, origin, share) in [
            ("owned", "p", None::<&str>),
            ("newer", "p", None),
            ("foreign", "other", None),
            ("private", "p", None),
            ("unknown", "unknown", None),
            ("global", "p", None),
            ("routed", "p", None),
            ("foreign-route", "p", None),
        ] {
            let mut task = crate::types::Task::new(id.into(), id.into());
            task.origin_project = Some(origin.into());
            tasks.add(&task).unwrap();
            let payload = serde_json::json!({"id":id, "origin_project":origin,
                "scope":if id == "global" {"global"} else {"project"}, "share":share});
            // Legacy fixture predates today's provenance enqueue guard.
            super::super::queue_ops::upsert_queue_row(
                &queue.conn.lock().unwrap(),
                EntityType::Task,
                id,
                SyncOperation::Upsert,
                Some(&payload.to_string()),
                "",
                match id {
                    "routed" => Some("p"),
                    "foreign-route" => Some("other"),
                    _ => None,
                },
            )
            .unwrap();
        }
        let mut entry = crate::types::Entry::new("entry".into(), "fixture".into());
        entry.origin_project = Some("p".into());
        crate::store::open_store_local(&root)
            .unwrap()
            .add(&entry)
            .unwrap();
        queue
            .enqueue(
                EntityType::Entry,
                "entry",
                SyncOperation::Upsert,
                Some(&serde_json::to_string(&entry).unwrap()),
            )
            .unwrap();
        let mut rule = crate::types::Rule::default();
        rule.id = "rule".into();
        rule.origin_project = Some("p".into());
        crate::store::open_rule_store_local(&root)
            .unwrap()
            .add(&rule)
            .unwrap();
        queue
            .enqueue(
                EntityType::Rule,
                "rule",
                SyncOperation::Upsert,
                Some(&serde_json::to_string(&rule).unwrap()),
            )
            .unwrap();
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
            vec!["owned", "routed", "entry", "rule", "edge"]
        );
        assert_eq!(
            serde_json::to_value(queue.list_all(100).unwrap()).unwrap(),
            serde_json::to_value(&before).unwrap()
        );
        let applied = queue.migrate_personal_to_team("t", "p", true).unwrap();
        assert_eq!(applied.iter().filter(|r| r.eligible).count(), 5);
        assert_eq!(
            queue.personal_row_count().unwrap(),
            13,
            "enqueue is not delivery"
        );
        let team = queue.pending_for_team("t", 100, 5).unwrap();
        assert_eq!(team.len(), 6);
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
        for id in ["owned", "routed"] {
            let sent = team.iter().find(|r| r.entity_id == id).unwrap();
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
        }
        queue
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE sync_queue SET project_id='other' WHERE entity_id='routed' AND team_id=''",
                [],
            )
            .unwrap();
        assert_eq!(
            queue.neutralize_team_only_personal("t", "p").unwrap(),
            1,
            "changed routing is not delivery proof"
        );
        queue
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE sync_queue SET project_id='p' WHERE entity_id='routed' AND team_id=''",
                [],
            )
            .unwrap();
        drop(queue);
        let reopened = SyncQueue::open(&root).unwrap();
        assert_eq!(reopened.neutralize_team_only_personal("t", "p").unwrap(), 1);
        assert_eq!(reopened.personal_row_count().unwrap(), 11);
    }
}
