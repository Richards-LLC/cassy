//! One ordinary authenticated invocation: durable gate -> actual local drain ->
//! submitting -> direct HTTP response -> validated immutable receipt storage.
use super::{
    binding_intake::{ChannelDisposition, IntakeJournal, IntakeScope, PauseOrigin},
    binding_reconciliation::{ReconciliationCode, ReconciliationRequest, ReconciliationResponse},
    binding_runtime::RuntimeInventory,
};
use serde::Serialize;
use std::{io::Read, path::Path, time::Duration};
use uuid::Uuid;

const RECONCILE_PATH: &str = "/api/coordination/bindings/reconcile";
const RESPONSE_LIMIT: u64 = 1024 * 1024;

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepairOutcome {
    Managed,
    Blocked,
    Unknown,
}
#[derive(Debug, Serialize)]
pub struct RepairReport {
    pub channel_id: String,
    pub operation_id: Uuid,
    pub activation_id: Option<String>,
    pub outcome: RepairOutcome,
    pub response: Option<ReconciliationResponse>,
    pub error: Option<String>,
}
impl RepairReport {
    fn blocked(scope: &IntakeScope, operation_id: Uuid, error: String) -> Self {
        Self {
            channel_id: scope.channel_id.clone(),
            operation_id,
            activation_id: None,
            outcome: RepairOutcome::Blocked,
            response: None,
            error: Some(error),
        }
    }
}
/// The fields/constructor are private to the adapter. A same-tuple parse-valid
/// body supplied by a caller cannot be attached to another local operation.
/// Cloud's invocation ID is server allocated, not the local pause UUID.
pub(super) struct CompletedInvocation {
    operation_id: Uuid,
    response: ReconciliationResponse,
}
impl CompletedInvocation {
    pub(super) fn operation_id(&self) -> Uuid {
        self.operation_id
    }
    pub(super) fn response(&self) -> &ReconciliationResponse {
        &self.response
    }
}

pub struct OrdinaryBindingClient {
    endpoint: String,
    token: String,
    agent: ureq::Agent,
}
impl OrdinaryBindingClient {
    pub fn new(endpoint: &str, token: &str) -> Result<Self, String> {
        let url = url::Url::parse(endpoint).map_err(|_| "invalid Cloud origin")?;
        let loopback = url.host_str().is_some_and(|host| {
            host.parse::<std::net::IpAddr>()
                .is_ok_and(|ip| ip.is_loopback())
        });
        if (url.scheme() != "https" && !(url.scheme() == "http" && loopback))
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
            || token.trim().is_empty()
            || token.contains(['\r', '\n'])
        {
            return Err("Cloud origin/ordinary user credential is unsupported".into());
        }
        Ok(Self {
            endpoint: endpoint.trim_end_matches('/').into(),
            token: token.into(),
            agent: ureq::AgentBuilder::new()
                .redirects(0)
                .timeout(Duration::from_secs(30))
                .build(),
        })
    }
    /// No active Cloud registration is created or used. Caller discovery is
    /// checked against the existing ordinary team-project read, never ensure().
    pub fn repair(&self, cas_dir: &Path, scope: &IntakeScope) -> Result<RepairReport, String> {
        scope.validate()?;
        let local =
            crate::cloud::CloudConfig::load_from_cas_dir(cas_dir).map_err(|e| e.to_string())?;
        if local.active_team_id().as_deref() != Some(scope.team_id.as_str())
            || crate::cloud::resolve_canonical_id(cas_dir).as_deref()
                != Some(scope.canonical_project_id.as_str())
        {
            return Err("reconciliation scope does not match this local project/team".into());
        }
        let journal = IntakeJournal::new(cas_dir);
        let state = journal.read()?;
        // A previous legacy_intake_active response is still waiting for the
        // host to go quiet. Failure to submit this retry cannot settle it.
        let preserve_pause = state.policies.iter().any(|policy| {
            &policy.scope == scope
                && matches!(
                    policy.disposition,
                    ChannelDisposition::Paused {
                        origin: Some(PauseOrigin::LegacyIntakeActive),
                    }
                )
        });
        // Reusing the scope's pause re-arms one an earlier blocked run released.
        let operation_id = state
            .pauses
            .iter()
            .rev()
            .find(|pause| &pause.scope == scope)
            .map_or_else(Uuid::new_v4, |pause| pause.operation_id);
        let pause = journal.pause(scope.clone(), operation_id, state.generation)?;
        // Nothing has left for Cloud in this attempt. Release only a pause
        // without a refusal origin, retaining a confirmed quiet-window hold.
        let blocked_before_submit = |error: String| -> Result<RepairReport, String> {
            let error = match if preserve_pause {
                Ok(())
            } else {
                journal.release_unsubmitted(scope)
            } {
                Ok(()) => error,
                Err(release) => format!("{error}; pause not released ({release}), rerun `cas update`"),
            };
            Ok(RepairReport::blocked(scope, pause.operation_id, error))
        };
        let inventory = match RuntimeInventory::discover(cas_dir).and_then(|inventory| {
            inventory.drain(scope, &pause)?;
            Ok(inventory)
        }) {
            Ok(inventory) => inventory,
            Err(error) => return blocked_before_submit(error),
        };
        drop(inventory);
        if let Err(error) = crate::cloud::TeamRegistration::new(
            &self.endpoint,
            &self.token,
            &scope.team_id,
            &scope.canonical_project_id,
        )
        .with_timeout(Duration::from_secs(20))
        .verify_registered()
        {
            return blocked_before_submit(error.reason);
        }
        let request = match ReconciliationRequest::new(
            &scope.team_id,
            &scope.canonical_project_id,
            &scope.channel_id,
        ) {
            Ok(request) => request,
            Err(error) => return blocked_before_submit(error.to_string()),
        };
        // This durable transition precedes sending. Transport/parser failures
        // keep it uncertain; no negative observation can prove that an
        // in-flight invocation had no effect.
        let attempt = match journal.mark_submitting(pause.operation_id) {
            Ok(attempt) => attempt,
            Err(error) => return blocked_before_submit(error),
        };
        let response = match self.invoke(&request) {
            Ok(response) => response,
            Err(error) => {
                if let Some(attempt) = attempt
                    && let Err(settle) = journal.settle_uncertain(attempt)
                {
                    tracing::warn!(%settle, "uncertain attempt left submitting");
                }
                return Ok(RepairReport {
                    channel_id: scope.channel_id.clone(),
                    operation_id: pause.operation_id,
                    activation_id: None,
                    outcome: RepairOutcome::Unknown,
                    response: None,
                    error: Some(error),
                });
            }
        };
        let activation_id = response.activation_id.clone();
        if response.code != ReconciliationCode::Completed {
            // Managed evidence (no attempt) stands; a refusal does not relax it.
            let uncertain = attempt.is_some_and(|attempt| {
                matches!(
                    journal
                        .settle_refusal(pause.operation_id, attempt, &response)
                        .unwrap_or(ChannelDisposition::Unknown),
                    ChannelDisposition::Unknown | ChannelDisposition::Submitting { .. }
                )
            });
            return Ok(RepairReport {
                channel_id: scope.channel_id.clone(),
                operation_id: pause.operation_id,
                activation_id,
                outcome: if uncertain {
                    RepairOutcome::Unknown
                } else {
                    RepairOutcome::Blocked
                },
                error: Some(response.guidance.clone()),
                response: Some(response),
            });
        }
        // Sole product constructor: only the direct response to THIS ordinary
        // HTTP invocation may cross the journal's completion boundary. A GET,
        // cached global-latest receipt or caller-supplied body cannot create it.
        let invocation = CompletedInvocation {
            operation_id: pause.operation_id,
            response: response.clone(),
        };
        if let Err(error) = journal.store_invocation(invocation) {
            return Ok(RepairReport {
                channel_id: scope.channel_id.clone(),
                operation_id: pause.operation_id,
                activation_id,
                outcome: RepairOutcome::Unknown,
                response: Some(response),
                error: Some(error),
            });
        }
        Ok(RepairReport {
            channel_id: scope.channel_id.clone(),
            operation_id: pause.operation_id,
            activation_id,
            outcome: RepairOutcome::Managed,
            response: Some(response),
            error: None,
        })
    }

    fn invoke(&self, request: &ReconciliationRequest) -> Result<ReconciliationResponse, String> {
        let response = self
            .agent
            .post(&format!("{}{RECONCILE_PATH}", self.endpoint))
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_json(serde_json::to_value(request).map_err(|e| e.to_string())?);
        let response = match response {
            Ok(response) | Err(ureq::Error::Status(_, response)) => response,
            Err(_) => {
                return Err("ordinary reconciliation transport uncertain; pause retained".into());
            }
        };
        let status = response.status();
        if !response.header("Cache-Control").is_some_and(|value| {
            value
                .split(',')
                .any(|directive| directive.trim().eq_ignore_ascii_case("no-store"))
        }) {
            return Err(
                "reconciliation response lacks no-store contract; outcome uncertain".into(),
            );
        }
        let mut bytes = Vec::new();
        response
            .into_reader()
            .take(RESPONSE_LIMIT + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "reconciliation body incomplete; outcome uncertain")?;
        if bytes.len() as u64 > RESPONSE_LIMIT {
            return Err("reconciliation response exceeds byte budget; outcome uncertain".into());
        }
        request
            .parse_response(status, &bytes)
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests;
