use cas_types::{
    Task, TaskType, Verification, VerificationDispatch, VerificationDispatchState, VerificationProvenance,
    VerificationStatus,
};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportProof {
    pub(crate) external_ref: Option<String>,
    pub(crate) artifacts: Vec<(String, String)>,
}

#[derive(Debug)]
pub(crate) struct ReportApproval {
    pub(crate) dispatch: VerificationDispatch,
    pub(crate) verdict: Verification,
    proof: ReportProof,
}

#[derive(Debug)]
pub(crate) enum ReportGate {
    Exempt,
    Bound(Box<ReportApproval>),
    Held,
    Mint { proof: ReportProof, stale: Option<String> },
}

impl ReportGate {
    pub(crate) fn covered(&self) -> bool {
        !matches!(self, Self::Exempt)
    }

    pub(crate) fn proof_to_record(&self) -> Option<String> {
        match self {
            Self::Mint { proof, .. } => serde_json::to_string(proof).ok(),
            Self::Bound(approval) => serde_json::to_string(&approval.proof).ok(),
            Self::Exempt | Self::Held => None,
        }
    }

    pub(crate) fn approval(&self) -> Option<&ReportApproval> {
        match self {
            Self::Bound(approval) => Some(approval),
            _ => None,
        }
    }

    pub(crate) fn approves(&self, dispatch_id: &str, verdict_id: &str) -> bool {
        self.approval().is_some_and(|approval| approval.dispatch.id == dispatch_id && approval.verdict.id == verdict_id)
    }

    pub(crate) fn retire_stale(self, root: &Path) -> Result<Self, String> {
        if let Self::Mint { stale: Some(dispatch), .. } = &self {
            cas_store::invalidate_verification_dispatch_for_proof_drift(root, dispatch).map_err(|error| format!(
                "⚠️ VERIFICATION REQUIRED\n\nStale verification dispatch {dispatch} of a no-code report could not be retired: {error}. \
                 Cassy does not close or merge the report without an approved independent verdict for the current report."
            ))?;
        }
        Ok(self)
    }

    pub(crate) fn recheck(&self, root: &Path, start: &Task, closing: &Task, current: &Task) -> Result<(), &'static str> {
        let Some(approved) = self.approval() else {
            return Ok(());
        };
        if current.external_ref != start.external_ref && current.external_ref != closing.external_ref {
            return Err("its report reference was edited");
        }
        let unchanged = connection(root)
            .and_then(|conn| read_report_verdict(&conn, true, current, closing))
            .is_ok_and(|now| now.approves(&approved.dispatch.id, &approved.verdict.id)
                && now.approval().is_some_and(|now| now.proof == approved.proof));
        if unchanged { Ok(()) } else { Err("its approved report proof no longer matches the report") }
    }
}

impl ReportProof {
    fn current(conn: &Connection, task: &Task) -> Result<Self, String> {
        let listing = crate::artifacts::proof::list_task_metadata_with_conn(conn, &task.id)
            .map_err(|error| format!("{error:#}"))?;
        if !listing.warnings.is_empty() {
            return Err(listing.warnings.join(", "));
        }
        let mut artifacts: Vec<_> = listing.committed().into_iter()
            .map(|artifact| (artifact.id, artifact.sha256))
            .collect();
        artifacts.sort();
        Ok(Self { external_ref: task.external_ref.clone(), artifacts })
    }

    fn recorded_with(conn: &Connection, dispatch_id: &str) -> Result<Option<Self>, String> {
        cas_store::verification_dispatch_reviewed_proof_with_conn(conn, dispatch_id)
            .map_err(|error| error.to_string())?
            .map(|recorded| serde_json::from_str(&recorded).map_err(|error| error.to_string()))
            .transpose()
    }

    pub(crate) fn recorded(root: &Path, dispatch_id: &str) -> Result<Option<Self>, String> {
        Self::recorded_with(&connection(root)?, dispatch_id)
    }
}

pub(crate) fn is_report(task: &Task) -> bool {
    task.execution_note.as_deref() == Some("no-code")
        && (task.task_type == TaskType::Spike || !task.demo_statement.trim().is_empty())
}

pub(crate) fn verification_on(config: &crate::config::Config) -> bool {
    use crate::harness_policy::{supervisor_harness_from_env, verification_policy, worker_harness_from_env};
    config.verification_enabled()
        && verification_policy(supervisor_harness_from_env(), worker_harness_from_env()).task_required()
}

fn connection(root: &Path) -> Result<Connection, String> {
    cas_store::SqliteVerificationStore::open(root).map_err(|error| error.to_string())?;
    let conn = Connection::open(root.join("cas.db")).map_err(|error| error.to_string())?;
    cas_store::shared_db::install_busy_handler(&conn).map_err(|error| error.to_string())?;
    Ok(conn)
}

pub(crate) fn require_report_verdict(
    root: &Path, verification_on: bool, stored: &Task, effective: &Task,
) -> Result<ReportGate, String> {
    if !verification_on {
        return Ok(ReportGate::Exempt);
    }
    let conn = connection(root).map_err(|error| unreadable(stored, "verification store", error))?;
    read_report_verdict(&conn, true, stored, effective)
}

fn unreadable(task: &Task, what: &str, error: String) -> String {
    format!(
        "⚠️ VERIFICATION REQUIRED\n\nTask {} is a no-code report and its {what} could not be read: {error}. \
         Cassy does not close or merge it without an approved independent verdict for the current report.",
        task.id
    )
}

pub(crate) fn read_report_verdict(
    conn: &Connection, verification_on: bool, stored: &Task, effective: &Task,
) -> Result<ReportGate, String> {
    if !verification_on {
        return Ok(ReportGate::Exempt);
    }
    let dispatch = conn.table_exists(None, "verification_dispatches")
        .map_err(cas_store::StoreError::Database)
        .and_then(|exists| if exists {
            cas_store::get_latest_verification_dispatch_with_conn(conn, &stored.id)
        } else {
            Ok(None)
        })
        .map_err(|error| unreadable(stored, "verification dispatch", error.to_string()))?;
    let recorded = match dispatch.as_ref() {
        Some(dispatch) => ReportProof::recorded_with(conn, &dispatch.id)
            .map_err(|error| unreadable(stored, "recorded proof", error))?,
        None => None,
    };
    if !(is_report(stored) || is_report(effective) || recorded.is_some()) {
        return Ok(ReportGate::Exempt);
    }
    let proof = ReportProof::current(conn, effective).map_err(|error| unreadable(stored, "artifact inventory", error))?;
    let Some(dispatch) = dispatch else {
        return Ok(ReportGate::Mint { proof, stale: None });
    };
    let active = matches!(dispatch.state, VerificationDispatchState::Pending | VerificationDispatchState::Claimed);
    if recorded.as_ref() != Some(&proof) {
        let stale = (active || recorded.is_some() && dispatch.state != VerificationDispatchState::Invalidated)
            .then_some(dispatch.id);
        return Ok(ReportGate::Mint { proof, stale });
    }
    if active {
        return Ok(ReportGate::Held);
    }
    if dispatch.state != VerificationDispatchState::Resolved {
        return Ok(ReportGate::Mint { proof, stale: None });
    }
    let verdict = cas_store::get_verification_for_dispatch_with_conn(conn, &dispatch.id)
        .map_err(|error| unreadable(stored, "verdict", error.to_string()))?;
    Ok(match verdict {
        Some(verdict)
            if verdict.status == VerificationStatus::Approved
                && verdict.provenance != VerificationProvenance::Legacy
                && verdict.dispatch_id.as_deref() == Some(dispatch.id.as_str()) =>
        {
            ReportGate::Bound(Box::new(ReportApproval { dispatch, verdict, proof }))
        }
        _ => ReportGate::Mint { proof, stale: None },
    })
}
