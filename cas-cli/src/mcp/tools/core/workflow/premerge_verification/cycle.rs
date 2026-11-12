//! Approval identity is the dispatch's delivery cycle; events only carry its stamp.
use super::*;
use cas_types::{
    Event, EventEntityType, EventType, VerificationProofBoundary, VerificationReceipt,
};
use rusqlite::{Connection, OptionalExtension, params};

fn connection(root: &Path) -> Result<Connection, String> {
    crate::qa_batch::release_close::read_connection(root)
}

pub(crate) fn current(
    conn: &Connection,
    task: &Task,
    anchor: &str,
) -> Result<Option<String>, String> {
    if !conn
        .table_exists(None, "delivery_cycles")
        .map_err(|e| e.to_string())?
    {
        return Ok(None);
    }
    let row: Option<(String, String)> = conn
        .query_row(
            "SELECT cycle_id, anchors FROM delivery_cycles WHERE task_id=?1 AND ended_at IS NULL
         AND cycle>=?2 ORDER BY cycle DESC LIMIT 1",
            params![task.id, task.deliverables.delivery_cycle],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(|e| e.to_string())?;
    row.map(|(id, anchors)| {
        let anchors: Vec<String> = serde_json::from_str(&anchors).map_err(|e| e.to_string())?;
        Ok(anchors.iter().any(|tip| tip == anchor).then_some(id))
    })
    .transpose()
    .map(Option::flatten)
}

/// Transaction receipts retain their protocol. Their task and delivered tip
/// must map to exactly one recorded cycle, never a guessed current cycle.
pub(crate) fn receipt_cycle(
    conn: &Connection,
    dispatch: &VerificationDispatch,
) -> Result<Option<String>, String> {
    match dispatch.receipt()? {
        VerificationReceipt::None => Ok(None),
        VerificationReceipt::DeliveryCycle(id) => Ok(Some(id.into())),
        VerificationReceipt::TransactionReceipt {
            receipt_id: id,
            transaction_id,
        } => {
            let (task, tip): (String, String) = conn
                .query_row(
                    "SELECT t.task_id,r.commit_sha FROM worker_delivery_transactions t
                 JOIN worker_completion_receipts r ON r.id=t.receipt_id AND r.task_id=t.task_id
                 WHERE t.id=?1 AND t.receipt_id=?2 AND t.task_id=?3",
                    params![transaction_id, id, dispatch.task_id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .map_err(|e| format!("TRANSACTION_CYCLE_MAPPING: {e}"))?;
            let mut rows = conn
                .prepare("SELECT cycle_id,anchors FROM delivery_cycles WHERE task_id=?1")
                .map_err(|e| e.to_string())?;
            let rows = rows
                .query_map([task], |r| {
                    Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))
                })
                .map_err(|e| e.to_string())?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| e.to_string())?;
            let mut matches = Vec::new();
            for (cycle, anchors) in rows {
                let anchors: Vec<String> =
                    serde_json::from_str(&anchors).map_err(|e| e.to_string())?;
                if anchors.contains(&tip) {
                    matches.push(cycle);
                }
            }
            if matches.len() != 1 {
                return Err(format!(
                    "TRANSACTION_CYCLE_MAPPING: expected one cycle, found {}",
                    matches.len()
                ));
            }
            Ok(matches.pop())
        }
    }
}

pub(crate) fn boundary(
    root: &Path,
    task: &Task,
    proof: RepositoryProofBoundary,
) -> Result<VerificationProofBoundary, String> {
    let mut boundary = VerificationProofBoundary::task_at(proof);
    if let Some(anchor) = task.deliverables.factory_branch_anchor.as_deref() {
        boundary.receipt_id =
            Some(current(&connection(root)?, task, anchor)?.ok_or("DELIVERY_CYCLE_MISSING")?);
    }
    Ok(boundary)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct Stamp {
    pub cycle_id: String,
    pub approved_anchor: String,
    pub dispatch_id: String,
    pub verdict_id: String,
    #[serde(default)]
    pub registry_version: Option<crate::gates::RegistryVersion>,
}

pub(crate) fn stamp(conn: &Connection, verdict: &str) -> Result<Option<Stamp>, String> {
    if !conn
        .table_exists(None, "events")
        .map_err(|e| e.to_string())?
    {
        return Ok(None);
    }
    let metadata: Option<String> = conn.query_row(
        "SELECT metadata FROM events WHERE event_type='verification_added' AND entity_type='verification'
         AND entity_id=?1 ORDER BY id LIMIT 1", [verdict], |r| r.get(0),
    ).optional().map_err(|e| e.to_string())?.flatten();
    let Some(metadata) = metadata else {
        return Ok(None);
    };
    let value: serde_json::Value = serde_json::from_str(&metadata).map_err(|e| e.to_string())?;
    if value.get("cycle_id").is_none() {
        return Ok(None);
    }
    serde_json::from_value(value)
        .map(Some)
        .map_err(|e| e.to_string())
}

/// The existing VerificationAdded event is committed with its verdict. A
/// dispatch created yesterday gets today's version only when approval is written.
pub(crate) fn event(
    conn: &Connection,
    root: &Path,
    task: &Task,
    dispatch: &VerificationDispatch,
    verdict: &Verification,
) -> Result<Event, String> {
    let mut metadata = serde_json::json!({"task_id":task.id,"status":verdict.status.storage_status(),
        "verification_type":verdict.verification_type.to_string(),"provenance":verdict.provenance.to_string(),
        "capability_id":verdict.capability_id,"dispatch_id":dispatch.id,"verdict_id":verdict.id});
    if verdict.status == VerificationStatus::Approved {
        if let Some(cycle) = receipt_cycle(conn, dispatch)? {
            let anchor = if let Some(candidate) = candidate_for_dispatch(root, dispatch)? {
                candidate.source_sha
            } else if let Some(peer) = dispatch
                .repository
                .as_ref()
                .and_then(|r| r.peer_premerge.as_ref())
            {
                peer.receipt.source_sha.clone()
            } else if let VerificationReceipt::TransactionReceipt { receipt_id: id, .. } =
                dispatch.receipt()?
            {
                conn.query_row(
                    "SELECT commit_sha FROM worker_completion_receipts WHERE id=?1",
                    [id],
                    |r| r.get(0),
                )
                .map_err(|e| e.to_string())?
            } else {
                task.deliverables
                    .factory_branch_anchor
                    .clone()
                    .ok_or("APPROVAL_ANCHOR_MISSING")?
            };
            let stamp = Stamp {
                cycle_id: cycle,
                approved_anchor: anchor,
                dispatch_id: dispatch.id.clone(),
                verdict_id: verdict.id.clone(),
                registry_version: if let Some(peer) = dispatch
                    .repository
                    .as_ref()
                    .and_then(|r| r.peer_premerge.as_ref())
                {
                    peer.receipt
                        .registry_version
                        .clone()
                        .map(serde_json::from_value)
                        .transpose()
                        .map_err(|e| e.to_string())?
                } else {
                    Some(crate::gates::registry_version_in_force())
                },
            };
            metadata.as_object_mut().unwrap().extend(
                serde_json::to_value(stamp)
                    .map_err(|e| e.to_string())?
                    .as_object()
                    .unwrap()
                    .clone(),
            );
        }
    }
    Ok(Event::new(
        EventType::VerificationAdded,
        EventEntityType::Verification,
        &verdict.id,
        format!(
            "Verification {}: {} - {}",
            verdict.status, task.id, verdict.summary
        ),
    )
    .with_metadata(metadata))
}

/// Resolve by dispatch receipt_id first. A newer task-level dispatch cannot
/// conceal this cycle's approval. Events cannot create approval authority.
pub(crate) struct ApprovalMatch {
    pub legacy: bool,
    pub registry_version: Option<crate::gates::RegistryVersion>,
}

pub(crate) fn approved(
    root: &Path,
    repo: &Path,
    task: &Task,
    anchor: &str,
    published: &str,
) -> Result<Option<ApprovalMatch>, String> {
    let conn = connection(root)?;
    if !conn
        .table_exists(None, "verification_dispatches")
        .map_err(|e| e.to_string())?
    {
        return Ok(None);
    }
    let cycle = current(&conn, task, anchor)?;
    let mut query = conn
        .prepare(
            "SELECT id FROM verification_dispatches WHERE task_id=?1 AND state='resolved'
        AND (receipt_id=?2 OR delivery_transaction_id IS NOT NULL OR receipt_id IS NULL)
        ORDER BY requested_at DESC,id DESC",
        )
        .map_err(|e| e.to_string())?;
    let ids = query
        .query_map(params![task.id, cycle], |r| r.get::<_, String>(0))
        .map_err(|e| e.to_string())?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| e.to_string())?;
    for id in ids {
        let dispatch = cas_store::get_verification_dispatch_with_conn(&conn, &id)
            .map_err(|e| e.to_string())?;
        let verdict:Option<(String,String)>=conn.query_row("SELECT id,status FROM verifications WHERE dispatch_id=?1 AND task_id=?2 ORDER BY created_at DESC,id DESC LIMIT 1",
            params![id,task.id],|r|Ok((r.get(0)?,r.get(1)?))).optional().map_err(|e|e.to_string())?;
        let Some((verdict, _status)) = verdict.filter(|(_, status)| status == "approved") else {
            continue;
        };
        match receipt_cycle(&conn, &dispatch)? {
            Some(bound) => {
                if cycle.as_deref() != Some(bound.as_str()) {
                    continue;
                }
                if let Some(stamp) = stamp(&conn, &verdict)? {
                    if stamp.cycle_id == bound
                        && stamp.approved_anchor == anchor
                        && stamp.dispatch_id == id
                        && stamp.verdict_id == verdict
                    {
                        return Ok(Some(ApprovalMatch {
                            legacy: false,
                            registry_version: stamp.registry_version,
                        }));
                    }
                }
            }
            None => {
                // Compatibility has one binding: validated candidate source
                // plus a reviewed tree actually present in published history.
                if let Some(candidate) = candidate_for_dispatch(root, &dispatch)? {
                    if candidate.source_sha != anchor {
                        continue;
                    }
                    let trees = Command::new("git")
                        .current_dir(repo)
                        .args(["log", "--format=%T", published, "--"])
                        .output()
                        .map_err(|e| e.to_string())?;
                    if trees.status.success()
                        && String::from_utf8_lossy(&trees.stdout)
                            .lines()
                            .any(|tree| tree == candidate.tree)
                    {
                        tracing::info!(task_id=%task.id,dispatch_id=%id,legacy_candidate_approval=true,"Legacy approval served");
                        return Ok(Some(ApprovalMatch {
                            legacy: true,
                            registry_version: None,
                        }));
                    }
                }
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_store::TaskStore;

    #[test]
    fn transaction_receipt_requires_exactly_one_cycle_cas_e01f() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        env.set("CAS_ROOT", &root);
        let mut task = Task::new("cas-wcr-cycle".into(), "Transaction mapping".into());
        task.origin_project = Some("fixture".into());
        task.deliverables.factory_branch_anchor = Some("tip".into());
        crate::store::open_task_store(&root)
            .unwrap()
            .add(&task)
            .unwrap();
        let conn = Connection::open(root.join("cas.db")).unwrap();
        conn.execute_batch(cas_store::DELIVERY_SCHEMA).unwrap();
        conn.execute("INSERT INTO worker_completion_receipts VALUES ('wcr-fixture',?1,'worker','worker','project:fixture','factory/w','tip','base','main','base','proof','scope',NULL,'2026-11-11T00:00:00Z')",[&task.id]).unwrap();
        conn.execute("INSERT INTO worker_delivery_transactions (id,receipt_id,task_id,state,created_at,updated_at) VALUES ('wdt-fixture','wcr-fixture',?1,'awaiting_merge','2026-11-11T00:00:00Z','2026-11-11T00:00:00Z')",[&task.id]).unwrap();
        let dispatch = cas_store::create_verification_dispatch_bound(
            &root,
            &task.id,
            "worker",
            "supervisor",
            &VerificationProofBoundary::delivery("wcr-fixture".into(), "wdt-fixture".into()),
            chrono::Utc::now() + chrono::Duration::hours(1),
            false,
        )
        .unwrap();
        let cycle = current(&conn, &task, "tip").unwrap().unwrap();
        assert_eq!(
            receipt_cycle(&conn, &dispatch).unwrap(),
            Some(cycle.clone())
        );
        conn.execute(
            "UPDATE delivery_cycles SET anchors='[]' WHERE cycle_id=?1",
            [&cycle],
        )
        .unwrap();
        assert!(receipt_cycle(&conn, &dispatch).is_err());
        conn.execute(
            "UPDATE delivery_cycles SET anchors='[\"tip\"]' WHERE cycle_id=?1",
            [&cycle],
        )
        .unwrap();
        conn.execute("INSERT INTO delivery_cycles SELECT 'dc-ambiguous',project,task_id,cycle+1,anchors,start_base,ended_at,created_at,updated_at FROM delivery_cycles WHERE cycle_id=?1",[&cycle]).unwrap();
        assert!(receipt_cycle(&conn, &dispatch).is_err());
    }

    #[test]
    fn rebind_keeps_original_registry_stamp_cas_e01f() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let dir = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(dir.path()).unwrap();
        env.set("CAS_ROOT", &root);
        let task = Task::new("cas-carry-stamp".into(), "Carry".into());
        crate::store::open_task_store(&root)
            .unwrap()
            .add(&task)
            .unwrap();
        let mut boundary = VerificationProofBoundary::task_at(RepositoryProofBoundary {
            peer_premerge: None,
            repository_root: "/repo".into(),
            worktree_root: "/repo/candidate".into(),
            head_commit: "head".into(),
            state_digest: "digest".into(),
            target_branch: None,
            anchor_commits: vec![],
        });
        boundary.receipt_id = Some("dc-cycle".into());
        let dispatch = cas_store::create_verification_dispatch_bound(
            &root,
            &task.id,
            "worker",
            "supervisor",
            &boundary,
            chrono::Utc::now() + chrono::Duration::hours(1),
            false,
        )
        .unwrap();
        let mut verdict =
            Verification::approved("ver-original".into(), task.id.clone(), "review".into());
        verdict.provenance = VerificationProvenance::System;
        verdict.dispatch_id = Some(dispatch.id.clone());
        cas_store::add_system_verification(&root, &verdict).unwrap();
        let conn = Connection::open(root.join("cas.db")).unwrap();
        cas_store::resolve_verification_dispatch_with_conn(
            &conn,
            &dispatch.id,
            "supervisor",
            None,
            true,
        )
        .unwrap();
        let historical: crate::gates::RegistryVersion =
            serde_json::from_value(serde_json::json!(["G-104"])).unwrap();
        let original = Stamp {
            cycle_id: "dc-cycle".into(),
            approved_anchor: "source".into(),
            dispatch_id: dispatch.id.clone(),
            verdict_id: verdict.id.clone(),
            registry_version: Some(historical.clone()),
        };
        let event = Event::new(
            EventType::VerificationAdded,
            EventEntityType::Verification,
            &verdict.id,
            "review",
        )
        .with_metadata(serde_json::to_value(original).unwrap());
        cas_store::record_event_with_conn(&conn, &event).unwrap();
        let (carried, new_verdict) = cas_store::rebind_approved_verification_dispatch(
            &root,
            &dispatch.id,
            &verdict.id,
            "worker",
            "supervisor",
            &boundary,
            "carry",
            "rebased-source",
        )
        .unwrap();
        let preserved = stamp(&conn, &new_verdict.id).unwrap().unwrap();
        assert_eq!(carried.receipt_id, dispatch.receipt_id);
        assert_eq!(preserved.registry_version, Some(historical));
        assert_eq!(preserved.approved_anchor, "rebased-source");
        assert_eq!(preserved.verdict_id, new_verdict.id);
        assert_eq!(preserved.dispatch_id, carried.id);
    }
}
