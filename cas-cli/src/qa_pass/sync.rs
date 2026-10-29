//! QA snapshots use the existing authenticated task transport. Ordinary note
//! writes cannot mint these reserved receipts; only QA lifecycle code does.
use std::path::Path;

use cas_types::{QaPass, Task};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const MARKER: &str = "QA_PASS_PROOF: ";

#[derive(Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pass: QaPass,
    changed_at: DateTime<Utc>,
}

pub(crate) fn record(cas_root: &Path, task_id: &str) -> anyhow::Result<()> {
    let store = crate::store::open_task_store(cas_root)?;
    let task = store.get(task_id)?;
    let existing = snapshots(&task.notes, task_id);
    for pass in cas_store::list_qa_passes(cas_root, task_id)? {
        if existing.iter().rev().find(|old| old.pass.id == pass.id)
            .is_some_and(|old| old.pass == pass) {
            continue;
        }
        let snapshot = Snapshot { pass, changed_at: Utc::now() };
        store.append_trusted_note(task_id, &format!("{MARKER}{}", serde_json::to_string(&snapshot)?))?;
    }
    Ok(())
}

fn snapshots(notes: &str, task_id: &str) -> Vec<Snapshot> {
    notes.lines().filter_map(|line| line.strip_prefix(MARKER))
        .filter_map(|json| serde_json::from_str::<Snapshot>(json).ok())
        .filter(|snapshot| snapshot.pass.task_id == task_id)
        .collect()
}

/// Remove receipt lines before ordinary task reconciliation can neutralize
/// them. The caller restores/imports them only after accepting the owner row.
pub(crate) fn take(notes: &mut String, task_id: &str) -> Vec<String> {
    let receipts = snapshots(notes, task_id).into_iter()
        .filter_map(|snapshot| serde_json::to_string(&snapshot).ok())
        .map(|json| format!("{MARKER}{json}")).collect();
    *notes = notes.lines().filter(|line| !line.starts_with(MARKER))
        .collect::<Vec<_>>().join("\n");
    receipts
}

pub(crate) fn import(cas_root: &Path, task: &Task, receipts: &[String]) -> anyhow::Result<()> {
    let store = crate::store::open_task_store_local(cas_root)?;
    for receipt in receipts {
        for snapshot in snapshots(receipt, &task.id) {
            cas_store::import_qa_pass(cas_root, &snapshot.pass, snapshot.changed_at)?;
        }
        store.append_trusted_note(&task.id, receipt)?;
    }
    Ok(())
}
