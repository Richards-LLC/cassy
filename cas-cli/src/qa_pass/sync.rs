//! QA snapshots use the existing authenticated task transport. Ordinary note
//! writes cannot mint these reserved receipts; only QA lifecycle code does.
use std::path::Path;

use cas_store::QaBatchReceipt;
use cas_types::{QaPass, Task};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const MARKER: &str = "QA_PASS_PROOF: ";
/// cas-6048: a batch receipt rides the same transport on each listed task.
const BATCH_MARKER: &str = "QA_BATCH_RECEIPT_PROOF: ";
/// A release verdict rides the same way under its own marker. A peer that predates
/// verdicts keeps the line as an ordinary note instead of refusing the task's sync.
const RELEASE_MARKER: &str = "RELEASE_VERDICT_PROOF: ";

fn receipt_marker(receipt: &QaBatchReceipt) -> &'static str {
    if receipt.schema_version == cas_store::RELEASE_VERDICT_SCHEMA { RELEASE_MARKER } else { BATCH_MARKER }
}

#[derive(Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pass: QaPass,
    changed_at: DateTime<Utc>,
}

pub(crate) fn record(cas_root: &Path, task_id: &str) -> anyhow::Result<()> {
    let store = crate::store::open_task_store(cas_root)?;
    let task = match store.get(task_id) {
        Ok(task) => task,
        // A QA child can arrive before its delivery in an incremental pull.
        // Claiming it remains valid; there is no owner row to publish yet.
        Err(cas_store::StoreError::TaskNotFound(_)) => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    let existing = snapshots(&task.notes, task_id);
    for pass in cas_store::list_qa_passes(cas_root, task_id)? {
        if existing
            .iter()
            .rev()
            .find(|old| old.pass.id == pass.id)
            .is_some_and(|old| old.pass == pass)
        {
            continue;
        }
        let snapshot = Snapshot {
            pass,
            changed_at: Utc::now(),
        };
        store.append_trusted_note(
            task_id,
            &format!("{MARKER}{}", serde_json::to_string(&snapshot)?),
        )?;
    }
    Ok(())
}

/// Publish a freshly recorded batch receipt on each task it lists, so a peer
/// machine's close gate can accept it after task sync (cas-6048).
pub(crate) fn record_batch(cas_root: &Path, receipt: &QaBatchReceipt) -> anyhow::Result<()> {
    let store = crate::store::open_task_store(cas_root)?;
    let line = format!("{}{}", receipt_marker(receipt), serde_json::to_string(receipt)?);
    for task_id in receipt.task_heads.keys() {
        store.append_trusted_note(task_id, &line)?;
    }
    Ok(())
}

fn batch_receipts(notes: &str, task_id: &str) -> Vec<QaBatchReceipt> {
    notes
        .lines()
        .filter_map(|line| line.strip_prefix(BATCH_MARKER).or_else(|| line.strip_prefix(RELEASE_MARKER)))
        .filter_map(|json| serde_json::from_str::<QaBatchReceipt>(json).ok())
        .filter(|receipt| receipt.task_heads.contains_key(task_id))
        .collect()
}

fn snapshots(notes: &str, task_id: &str) -> Vec<Snapshot> {
    notes
        .lines()
        .filter_map(|line| line.strip_prefix(MARKER))
        .filter_map(|json| serde_json::from_str::<Snapshot>(json).ok())
        .filter(|snapshot| snapshot.pass.task_id == task_id)
        .collect()
}

/// Remove receipt lines before ordinary task reconciliation can neutralize
/// them. The caller restores/imports them only after accepting the owner row.
pub fn take(notes: &mut String, task_id: &str) -> Vec<String> {
    let receipts = snapshots(notes, task_id)
        .into_iter()
        .filter_map(|snapshot| serde_json::to_string(&snapshot).ok())
        .map(|json| format!("{MARKER}{json}"))
        .chain(
            batch_receipts(notes, task_id)
                .into_iter()
                .map(|receipt| QaBatchReceipt {
                    imported: false,
                    ..receipt
                })
                .filter_map(|receipt| {
                    let json = serde_json::to_string(&receipt).ok()?;
                    Some(format!("{}{json}", receipt_marker(&receipt)))
                }),
        )
        .collect();
    *notes = notes
        .lines()
        .filter(|line| {
            !line.starts_with(MARKER) && !line.starts_with(BATCH_MARKER) && !line.starts_with(RELEASE_MARKER)
        })
        .collect::<Vec<_>>()
        .join("\n");
    receipts
}

pub fn import(cas_root: &Path, task: &Task, receipts: &[String]) -> anyhow::Result<()> {
    let store = crate::store::open_task_store_local(cas_root)?;
    let mut errors = Vec::new();
    for receipt in receipts {
        let mut accepted = true;
        for batch in batch_receipts(receipt, &task.id) {
            if let Err(error) = cas_store::import_qa_batch_receipt(cas_root, &batch) {
                errors.push(format!("{}: {error}", batch.id));
                accepted = false;
            }
        }
        for snapshot in snapshots(receipt, &task.id) {
            if let Err(error) =
                cas_store::import_qa_pass(cas_root, &snapshot.pass, snapshot.changed_at)
            {
                errors.push(format!("{}: {error}", snapshot.pass.id));
                accepted = false;
            }
        }
        if accepted {
            store.append_trusted_note(&task.id, receipt)?;
        }
    }
    anyhow::ensure!(errors.is_empty(), "{}", errors.join("; "));
    Ok(())
}
