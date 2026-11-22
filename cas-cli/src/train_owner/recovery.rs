//! Local operator recovery. This request is deliberately absent from MCP;
//! neither a supervisor override nor caller-provided provenance grants H-41.
use crate::config::operator_policy::InvocationContext;
use crate::mcp::CasCore;
use crate::worktree::git::train_authority::{OperatorClaim, Snapshot, TransferChecks};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Action { Force { successor: String, transfer_id: String }, AdoptTarget }

pub(crate) struct Request {
    pub action: Action,
    pub expected_authority: String,
    pub expected_generation: String,
    pub expected_target: String,
    pub request_id: String,
    pub reason: String,
    pub evidence: String,
    pub stopped_or_fenced: String,
}

impl CasCore {
    pub(crate) fn train_owner_recover(&self, train_id: &str, request: &Request) -> Result<Value, String> {
        let context = InvocationContext::from_process();
        let tasks = self.open_task_store().map_err(|e| e.to_string())?;
        let train = tasks.get(train_id).map_err(|e| e.to_string())?;
        let repo = crate::train_guardrails::train_repo_root(&self.cas_root, &train).map_err(|e| e.to_string())?;
        let checks = super::provider::GitHubChecks::new(&repo)?;
        self.train_owner_recover_checked(train_id, request, &context, &checks)
    }

    // Replaces only process context/provider I/O in tests. All scope checks,
    // registered successor resolution, Git transitions and projection are real.
    pub(crate) fn train_owner_recover_checked(&self, train_id: &str, request: &Request,
        context: &InvocationContext, checks: &impl TransferChecks) -> Result<Value, String>
    {
        // RED sabotage: H-41 call intentionally omitted until refusal is observed.
        let _ = context;
        if [&request.expected_authority, &request.expected_generation, &request.expected_target,
            &request.reason, &request.evidence, &request.stopped_or_fenced].iter().any(|s| s.trim().is_empty())
            || uuid::Uuid::parse_str(&request.request_id).is_err()
        {
            return Err("TRAIN_TRANSFER_OPERATOR_ATTESTATION_REQUIRED: exact scope and positive fencing evidence required".into());
        }
        let tasks = self.open_task_store().map_err(|e| e.to_string())?;
        let train = tasks.get(train_id).map_err(|e| e.to_string())?;
        if !crate::mcp::tools::core::task::release_train::is_release_train(&train) || train.is_terminal() {
            return Err("TRAIN_TRANSFER_CONFLICT: recovery requires an open release train".into());
        }
        let repo = crate::train_guardrails::train_repo_root(&self.cas_root, &train).map_err(|e| e.to_string())?;
        let snapshot = super::authority(&self.cas_root, &train)?
            .ok_or("TRAIN_TRANSFER_FENCE_UNAVAILABLE: enroll the train before recovering it")?;
        if snapshot.tip != request.expected_authority || snapshot.record.generation != request.expected_generation {
            return Err("TRAIN_TRANSFER_CONFLICT: expected authority or generation changed".into());
        }
        if snapshot.observed_target(&repo)? != request.expected_target {
            return Err("TRAIN_TRANSFER_TARGET_DIVERGED: expected target does not match remote".into());
        }
        let operator = operator_name()?;
        let project = snapshot.record.project.clone();
        let snapshot = snapshot.audited(json!({"request_id": request.request_id, "reason": request.reason,
            "train": train.id, "project": project, "operator_claim_trust": "unauthenticated",
            "operator": operator, "expected_authority": request.expected_authority,
            "expected_generation": request.expected_generation, "expected_target": request.expected_target,
            "observed_at": chrono::Utc::now()}));
        let claim = OperatorClaim { operator, reason: request.reason.clone(), evidence: request.evidence.clone(),
            stopped_or_fenced: request.stopped_or_fenced.clone() };
        let result: Snapshot = match &request.action {
            Action::Force { successor, transfer_id } => {
                let successor = super::transfer::resolve_successor(&self.cas_root, None, successor)?;
                snapshot.force_quiesce(&repo, &successor, transfer_id, &claim, checks)?
            }
            Action::AdoptTarget => snapshot.adopt_target(&repo, &request.expected_target, &claim, checks)?,
        };
        let current = tasks.get(train_id).map_err(|e| e.to_string())?;
        super::repair_projection(&self.cas_root, tasks.as_ref(), &current, &result)?;
        Ok(json!({"train": train_id, "authority": result.tip, "generation": result.record.generation,
            "phase": result.record.phase, "owner": result.record.identity.owner,
            "operator_claim_trust": "unauthenticated"}))
    }
}

fn operator_name() -> Result<String, String> {
    let output = std::process::Command::new("id").arg("-un").output().map_err(|e| e.to_string())?;
    let name = String::from_utf8(output.stdout).map_err(|e| e.to_string())?;
    if !output.status.success() || name.trim().is_empty() {
        return Err("TRAIN_TRANSFER_OPERATOR_ATTESTATION_REQUIRED: local operator identity unavailable".into());
    }
    Ok(name.trim().to_string())
}
