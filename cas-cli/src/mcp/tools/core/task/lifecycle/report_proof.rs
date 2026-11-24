//! A no-code report's review proof and the one gate every route to Closed or
//! to a merge asks before any exemption.
use cas_types::{Task, TaskType, VerificationDispatchState, VerificationProvenance, VerificationStatus};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ReportProof {
    pub(crate) external_ref: Option<String>,
    pub(crate) artifacts: Vec<(String, String)>,
}

#[derive(Debug)]
pub(crate) enum ReportGate {
    Exempt,
    Bound { dispatch: String, verdict: String, proof: ReportProof },
    Held,
    Mint(ReportProof),
}

impl ReportGate {
    pub(crate) fn covered(&self) -> bool {
        !matches!(self, Self::Exempt)
    }

    pub(crate) fn proof_to_record(&self) -> Option<String> {
        match self {
            Self::Mint(proof) | Self::Bound { proof, .. } => serde_json::to_string(proof).ok(),
            Self::Exempt | Self::Held => None,
        }
    }
}

impl ReportProof {
    pub(crate) fn current(root: &Path, task: &Task) -> Result<Self, String> {
        let listing = crate::artifacts::proof::list_task_metadata(root, &task.id)
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

    pub(crate) fn recorded(root: &Path, dispatch_id: &str) -> Result<Option<Self>, String> {
        cas_store::verification_dispatch_reviewed_proof(root, dispatch_id)
            .map_err(|error| error.to_string())?
            .map(|recorded| serde_json::from_str(&recorded).map_err(|error| error.to_string()))
            .transpose()
    }
}

fn is_report(task: &Task) -> bool {
    task.execution_note.as_deref() == Some("no-code")
        && (task.task_type == TaskType::Spike || !task.demo_statement.trim().is_empty())
}

fn verification_on(root: &Path) -> bool {
    use crate::harness_policy::{supervisor_harness_from_env, verification_policy, worker_harness_from_env};
    crate::config::Config::load(root).unwrap_or_default().verification_enabled()
        && verification_policy(supervisor_harness_from_env(), worker_harness_from_env()).task_required()
}

/// `stored` is the task as saved, `effective` the task this request would
/// save. A refusal is the G-68 text; callers tag and return it.
pub(crate) fn require_report_verdict(root: &Path, stored: &Task, effective: &Task) -> Result<ReportGate, String> {
    if !verification_on(root) {
        return Ok(ReportGate::Exempt);
    }
    let refuse = |unreadable: &str, error: String| format!(
        "⚠️ VERIFICATION REQUIRED\n\nTask {} is a no-code report and its {unreadable} could not be read: {error}. \
         Cassy does not close or merge it without an approved independent verdict for the current report.",
        stored.id
    );
    let dispatch = cas_store::get_latest_verification_dispatch(root, &stored.id)
        .map_err(|error| refuse("verification dispatch", error.to_string()))?;
    let recorded = match dispatch.as_ref() {
        Some(dispatch) => ReportProof::recorded(root, &dispatch.id).map_err(|error| refuse("recorded proof", error))?,
        None => None,
    };
    if !(is_report(stored) || is_report(effective) || recorded.is_some()) {
        return Ok(ReportGate::Exempt);
    }
    let current = ReportProof::current(root, effective).map_err(|error| refuse("artifact inventory", error))?;
    let Some(dispatch) = dispatch else {
        return Ok(ReportGate::Mint(current));
    };
    let active = matches!(dispatch.state, VerificationDispatchState::Pending | VerificationDispatchState::Claimed);
    let Some(recorded) = recorded else {
        return Ok(if active { ReportGate::Held } else { ReportGate::Mint(current) });
    };
    if recorded != current {
        cas_store::invalidate_verification_dispatch_for_proof_drift(root, &dispatch.id)
            .map_err(|error| refuse("stale verification dispatch", error.to_string()))?;
        return Ok(ReportGate::Mint(current));
    }
    if active {
        return Ok(ReportGate::Held);
    }
    if dispatch.state != VerificationDispatchState::Resolved {
        return Ok(ReportGate::Mint(current));
    }
    let verdict = cas_store::get_verification_for_dispatch(root, &dispatch.id)
        .map_err(|error| refuse("verdict", error.to_string()))?;
    Ok(match verdict {
        Some(verdict)
            if verdict.status == VerificationStatus::Approved
                && verdict.provenance != VerificationProvenance::Legacy
                && verdict.dispatch_id.as_deref() == Some(dispatch.id.as_str()) =>
        {
            ReportGate::Bound { dispatch: dispatch.id, verdict: verdict.id, proof: current }
        }
        _ => ReportGate::Mint(current),
    })
}
