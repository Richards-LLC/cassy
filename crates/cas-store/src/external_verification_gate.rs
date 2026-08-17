//! Fail-closed contract for supervisor-owned external verification.
//!
//! This module deliberately knows nothing about a provider credential or
//! transport. The caller must have reserved a [`DelegationReceipt`] before an
//! upstream run begins; this boundary validates the provider's structured
//! answer and persists the result through that receipt.

use std::collections::HashSet;

use cas_types::AgentRole;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    DelegationReceipt, DelegationVerdict, Result, SqliteDelegationReceiptStore, StoreError,
};

pub const EXTERNAL_PRODUCTION_VERIFICATION_GATE: &str = "external_production_verification";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RequiredCheck {
    pub name: String,
    pub expected: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalVerificationRequest {
    /// Authority resolved by CAS from the registered caller, never caller JSON.
    pub caller_role: AgentRole,
    /// Local proof must exist before an external check can be requested.
    pub local_proof_reference: String,
    pub required_checks: Vec<RequiredCheck>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalVerificationOutcome {
    /// A provider response formatted according to the response schema.
    Response,
    WaitTimedOut,
    RequiresAction,
    ThreadBusy,
    InsufficientScope,
    RateLimited,
    Cancelled,
    TransportFailure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateAssessment {
    verdict: DelegationVerdict,
    passing: bool,
}

impl GateAssessment {
    pub fn verdict(&self) -> DelegationVerdict {
        self.verdict
    }

    pub fn is_passing(&self) -> bool {
        self.passing
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderResponse {
    verdict: ProviderVerdict,
    checks: Vec<ProviderCheck>,
    limitations: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ProviderVerdict {
    Pass,
    Fail,
    Inconclusive,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderCheck {
    name: String,
    expected: String,
    observed: String,
    evidence: String,
}

/// Validate supervisor authority and the local proof precondition.
pub fn authorize_request(request: &ExternalVerificationRequest) -> Result<()> {
    if request.caller_role != AgentRole::Supervisor {
        return Err(StoreError::Parse(
            "external production verification is supervisor-only".to_string(),
        ));
    }
    if request.local_proof_reference.trim().is_empty() {
        return Err(StoreError::Parse(
            "external production verification requires local proof first".to_string(),
        ));
    }
    if request.required_checks.is_empty()
        || request
            .required_checks
            .iter()
            .any(|check| check.name.trim().is_empty() || check.expected.trim().is_empty())
    {
        return Err(StoreError::Parse(
            "external production verification requires named expected checks".to_string(),
        ));
    }
    Ok(())
}

/// Fail closed while converting an upstream response into CAS's verdict.
///
/// Schema errors and policy violations intentionally become `Malformed`, not
/// an error that a caller could accidentally interpret as a successful gate.
pub fn assess_response(request: &ExternalVerificationRequest, response: Value) -> GateAssessment {
    if authorize_request(request).is_err() {
        return GateAssessment {
            verdict: DelegationVerdict::Malformed,
            passing: false,
        };
    }
    let Ok(response) = serde_json::from_value::<ProviderResponse>(response) else {
        return GateAssessment {
            verdict: DelegationVerdict::Malformed,
            passing: false,
        };
    };
    match response.verdict {
        ProviderVerdict::Fail => GateAssessment {
            verdict: DelegationVerdict::Fail,
            passing: false,
        },
        ProviderVerdict::Inconclusive => GateAssessment {
            verdict: DelegationVerdict::Inconclusive,
            passing: false,
        },
        ProviderVerdict::Pass if response_passes_policy(request, &response) => GateAssessment {
            verdict: DelegationVerdict::Pass,
            passing: true,
        },
        ProviderVerdict::Pass => GateAssessment {
            verdict: DelegationVerdict::Malformed,
            passing: false,
        },
    }
}

pub fn assessment_for_outcome(outcome: ExternalVerificationOutcome) -> GateAssessment {
    let verdict = match outcome {
        ExternalVerificationOutcome::Response => DelegationVerdict::Malformed,
        ExternalVerificationOutcome::WaitTimedOut => DelegationVerdict::WaitTimedOut,
        ExternalVerificationOutcome::RequiresAction => DelegationVerdict::RequiresAction,
        ExternalVerificationOutcome::ThreadBusy => DelegationVerdict::ThreadBusy,
        ExternalVerificationOutcome::InsufficientScope => DelegationVerdict::InsufficientScope,
        ExternalVerificationOutcome::RateLimited => DelegationVerdict::RateLimited,
        ExternalVerificationOutcome::Cancelled => DelegationVerdict::Cancelled,
        ExternalVerificationOutcome::TransportFailure => DelegationVerdict::TransportFailure,
    };
    GateAssessment {
        verdict,
        passing: false,
    }
}

/// Persist a final external-gate result. `RequiresAction` is final here: a
/// later supervisor action must reserve a new request rather than auto-run.
pub fn record_assessment(
    store: &SqliteDelegationReceiptStore,
    receipt_id: &str,
    assessment: &GateAssessment,
    settled_amount: u64,
    evidence_reference: &str,
) -> Result<DelegationReceipt> {
    store.record_terminal(
        receipt_id,
        assessment.verdict,
        settled_amount,
        evidence_reference,
    )
}

fn response_passes_policy(
    request: &ExternalVerificationRequest,
    response: &ProviderResponse,
) -> bool {
    if response.checks.is_empty() {
        return false;
    }
    let mut seen = HashSet::new();
    for check in &response.checks {
        if check.name.trim().is_empty()
            || check.expected.trim().is_empty()
            || check.observed.trim().is_empty()
            || check.evidence.trim().is_empty()
            || !seen.insert(check.name.as_str())
        {
            return false;
        }
    }
    if request.required_checks.iter().any(|required| {
        !response
            .checks
            .iter()
            .any(|actual| actual.name == required.name && actual.expected == required.expected)
    }) {
        return false;
    }
    !response.limitations.iter().any(|limitation| {
        let limitation = limitation.to_ascii_lowercase();
        request.required_checks.iter().any(|required| {
            limitation.contains(&required.name.to_ascii_lowercase())
                || limitation.contains(&required.expected.to_ascii_lowercase())
        })
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{DelegationBudget, DelegationReserveOutcome, DelegationReserveRequest};
    use tempfile::TempDir;

    fn request() -> ExternalVerificationRequest {
        ExternalVerificationRequest {
            caller_role: AgentRole::Supervisor,
            local_proof_reference: "local://test-proof".into(),
            required_checks: vec![RequiredCheck {
                name: "homepage loads".into(),
                expected: "200 OK".into(),
            }],
        }
    }
    fn pass_response() -> Value {
        serde_json::json!({"verdict":"pass","checks":[{"name":"homepage loads","expected":"200 OK","observed":"200 OK","evidence":"https://example.test/"}],"limitations":[]})
    }
    fn receipt_store() -> (TempDir, SqliteDelegationReceiptStore, String) {
        let dir = TempDir::new().unwrap();
        let store = SqliteDelegationReceiptStore::open(dir.path()).unwrap();
        let reservation = DelegationReserveRequest {
            factory_session_id: "factory".into(),
            epic_id: "epic".into(),
            task_id: "task".into(),
            gate_kind: EXTERNAL_PRODUCTION_VERIFICATION_GATE.into(),
            request_digest: "digest".into(),
            reserved_amount: 1,
        };
        let budget = DelegationBudget {
            max_per_run: 1,
            max_active_per_factory_session: 1,
            max_active_per_epic: 1,
        };
        let DelegationReserveOutcome::Created(receipt) =
            store.reserve_or_resume(&reservation, &budget).unwrap()
        else {
            panic!("new receipt");
        };
        (dir, store, receipt.id)
    }
    fn assert_persisted_nonpass(outcome: ExternalVerificationOutcome, verdict: DelegationVerdict) {
        let (_dir, store, receipt_id) = receipt_store();
        let assessment = assessment_for_outcome(outcome);
        assert_eq!(assessment.verdict, verdict);
        let stored = record_assessment(
            &store,
            &receipt_id,
            &assessment,
            1,
            "delegation-evidence://run",
        )
        .unwrap();
        assert!(!assessment.passing);
        assert_eq!(stored.terminal_verdict, Some(verdict));
        assert_eq!(stored.state, crate::DelegationReceiptState::Completed);
    }

    #[test]
    fn valid_supervisor_pass_meeting_policy_is_passing() {
        assert!(assess_response(&request(), pass_response()).passing);
    }
    #[test]
    fn worker_is_denied_before_external_gate() {
        let mut r = request();
        r.caller_role = AgentRole::Worker;
        assert!(authorize_request(&r).is_err());
    }
    #[test]
    fn missing_local_proof_is_denied_before_external_gate() {
        let mut r = request();
        r.local_proof_reference.clear();
        assert!(authorize_request(&r).is_err());
    }
    #[test]
    fn fail_is_a_durable_nonpassing_receipt() {
        let (_dir, store, id) = receipt_store();
        let assessment = assess_response(
            &request(),
            serde_json::json!({"verdict":"fail","checks":[],"limitations":[]}),
        );
        let stored =
            record_assessment(&store, &id, &assessment, 1, "delegation-evidence://run").unwrap();
        assert_eq!(stored.terminal_verdict, Some(DelegationVerdict::Fail));
    }
    #[test]
    fn inconclusive_is_a_durable_nonpassing_receipt() {
        let (_dir, store, id) = receipt_store();
        let assessment = assess_response(
            &request(),
            serde_json::json!({"verdict":"inconclusive","checks":[],"limitations":[]}),
        );
        let stored =
            record_assessment(&store, &id, &assessment, 1, "delegation-evidence://run").unwrap();
        assert_eq!(
            stored.terminal_verdict,
            Some(DelegationVerdict::Inconclusive)
        );
    }
    #[test]
    fn wait_timed_out_is_a_durable_nonpassing_receipt() {
        let (_dir, store, id) = receipt_store();
        let stored = store.record_timeout(&id, "run-1").unwrap();
        assert_eq!(
            stored.terminal_verdict,
            Some(DelegationVerdict::WaitTimedOut)
        );
    }
    #[test]
    fn requires_action_is_a_durable_nonpassing_receipt() {
        assert_persisted_nonpass(
            ExternalVerificationOutcome::RequiresAction,
            DelegationVerdict::RequiresAction,
        );
    }
    #[test]
    fn requires_action_never_auto_continues_to_pass() {
        let (_dir, store, receipt_id) = receipt_store();
        let required_action = assessment_for_outcome(ExternalVerificationOutcome::RequiresAction);
        record_assessment(
            &store,
            &receipt_id,
            &required_action,
            1,
            "delegation-evidence://action-required",
        )
        .unwrap();
        let attempted_auto_continue = GateAssessment {
            verdict: DelegationVerdict::Pass,
            passing: true,
        };
        let persisted = record_assessment(
            &store,
            &receipt_id,
            &attempted_auto_continue,
            1,
            "delegation-evidence://unexpected-pass",
        )
        .unwrap();
        assert_eq!(
            persisted.terminal_verdict,
            Some(DelegationVerdict::RequiresAction),
            "a completed requires_action receipt may only be followed by a new explicit request"
        );
    }
    #[test]
    fn thread_busy_is_a_durable_nonpassing_receipt() {
        assert_persisted_nonpass(
            ExternalVerificationOutcome::ThreadBusy,
            DelegationVerdict::ThreadBusy,
        );
    }
    #[test]
    fn insufficient_scope_is_a_durable_nonpassing_receipt() {
        assert_persisted_nonpass(
            ExternalVerificationOutcome::InsufficientScope,
            DelegationVerdict::InsufficientScope,
        );
    }
    #[test]
    fn rate_limit_is_a_durable_nonpassing_receipt() {
        assert_persisted_nonpass(
            ExternalVerificationOutcome::RateLimited,
            DelegationVerdict::RateLimited,
        );
    }
    #[test]
    fn cancellation_is_a_durable_nonpassing_receipt() {
        assert_persisted_nonpass(
            ExternalVerificationOutcome::Cancelled,
            DelegationVerdict::Cancelled,
        );
    }
    #[test]
    fn malformed_output_is_a_durable_nonpassing_receipt() {
        let (_dir, store, id) = receipt_store();
        let assessment = assess_response(&request(), serde_json::json!({"verdict":"pass"}));
        let stored =
            record_assessment(&store, &id, &assessment, 1, "delegation-evidence://run").unwrap();
        assert_eq!(stored.terminal_verdict, Some(DelegationVerdict::Malformed));
    }
    #[test]
    fn transport_failure_is_a_durable_nonpassing_receipt() {
        assert_persisted_nonpass(
            ExternalVerificationOutcome::TransportFailure,
            DelegationVerdict::TransportFailure,
        );
    }
    #[test]
    fn limitations_conflicting_with_required_check_fail_closed() {
        let mut response = pass_response();
        response["limitations"] = serde_json::json!(["Could not confirm homepage loads"]);
        assert_eq!(
            assess_response(&request(), response).verdict,
            DelegationVerdict::Malformed
        );
    }
}
