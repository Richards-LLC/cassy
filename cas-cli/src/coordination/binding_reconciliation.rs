//! Pure wire validation for spec-b489's ordinary-user binding reconciliation.
//!
//! This separate version never extends `cassy.coordination/v1`. It performs no
//! I/O, grants no reader/claim authority, and does not pause or activate a runtime.

use serde::{Deserialize, Deserializer, Serialize};

pub const SCHEMA: &str = "cassy.binding-reconciliation/v1";

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid binding reconciliation wire: {0}")]
pub struct WireError(&'static str);

/// Use a Cloud-resolved canonical project, not an alias guessed by the client.
/// Exact identities are preserved; enrollment supplies the workspace in replies.
#[derive(Debug, Clone, Serialize)]
pub struct ReconciliationRequest {
    schema: &'static str,
    team_id: String,
    project_id: String,
    channel_id: String,
}

impl ReconciliationRequest {
    pub fn new(
        team_id: &str,
        canonical_project_id: &str,
        channel_id: &str,
    ) -> Result<Self, WireError> {
        Ok(Self {
            schema: SCHEMA,
            team_id: team_id.into(),
            project_id: canonical_project_id.into(),
            channel_id: channel_id.into(),
        })
    }

    /// Validate a response against this exact request and its HTTP status.
    /// Errors, including malformed 2xx, leave the mutation outcome unknown;
    /// they never authorize resuming paused intake or falling back to admin/v1.
    pub fn parse_response(
        &self,
        _status: u16,
        _body: &[u8],
    ) -> Result<ReconciliationResponse, WireError> {
        Err(WireError("response validation is not implemented"))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationCode {
    Completed,
    InvalidRequest,
    UnsupportedVersion,
    Unauthorized,
    Forbidden,
    UnknownProject,
    BindingConflict,
    MappingConflict,
    MappingNotFound,
    InstallationMismatch,
    ChannelUnavailable,
    LegacyIntakeActive,
    BindingUnevidenced,
    RateLimited,
    AuthorityUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NextAction {
    None,
    Authenticate,
    FixRequest,
    RestoreMembership,
    LinkTeamProject,
    UpgradeClient,
    InviteViolet,
    StopLegacyIntake,
    ExplicitRelink,
    OperatorRepair,
    RetryLater,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BindingMode {
    Legacy,
    Activating,
    Managed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrictActivation {
    Evidenced,
    Unevidenced,
    Refused,
    NotRequested,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationOutcome {
    Activated,
    AlreadyManaged,
    Refused,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindingScope {
    pub team_id: String,
    pub project_id: String,
    pub workspace_id: String,
    pub channel_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Binding {
    pub id: String,
    pub provider: String,
    pub team_id: String,
    pub project_id: String,
    pub workspace_id: String,
    pub channel_id: String,
    pub mode: BindingMode,
    pub verified: bool,
    pub fence_generation: u64,
    #[serde(deserialize_with = "required_nullable")]
    pub activated_at: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub legacy_fenced_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActivationReceipt {
    pub id: String,
    pub outcome: ActivationOutcome,
    pub prior_mode: BindingMode,
    pub fence_generation: u64,
    pub attempted_at: String,
    pub quiescence: Quiescence,
    pub settled: ReceiptSettlement,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Quiescence {
    pub quiet_window_secs: u64,
    pub attested_v1_consumers: Vec<String>,
    pub blocking_v1_leases: u64,
    pub blocking_v1_consumers: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReceiptSettlement {
    pub migrated_v1_rows: u64,
    pub retired_v1_rows: u64,
    pub invalidated_v1_leases: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settlement {
    pub migrated: u64,
    pub retired: u64,
    pub invalidated_leases: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Blockers {
    pub unexpired_v1_leases: u64,
    pub live_v1_consumers: Vec<String>,
    pub live_v1_consumers_count: u64,
    pub consumers_truncated: bool,
}

/// Only `ReconciliationRequest::parse_response` establishes wire validity.
/// Even evidenced completion is configuration evidence, not work authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReconciliationResponse {
    pub schema: String,
    pub code: ReconciliationCode,
    pub reason: String,
    pub guidance: String,
    #[serde(deserialize_with = "required_nullable")]
    pub scope: Option<BindingScope>,
    #[serde(deserialize_with = "required_nullable")]
    pub binding: Option<Binding>,
    #[serde(deserialize_with = "required_nullable")]
    pub strict_activation: Option<StrictActivation>,
    #[serde(deserialize_with = "required_nullable")]
    pub activation_id: Option<String>,
    #[serde(deserialize_with = "required_nullable")]
    pub fence_receipt: Option<ActivationReceipt>,
    #[serde(deserialize_with = "required_nullable")]
    pub latest_attempt: Option<ActivationReceipt>,
    #[serde(deserialize_with = "required_nullable")]
    pub settled: Option<Settlement>,
    #[serde(deserialize_with = "required_nullable")]
    pub blockers: Option<Blockers>,
    #[serde(deserialize_with = "required_nullable")]
    pub retry_after_secs: Option<u64>,
    pub next_action: NextAction,
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[cfg(test)]
mod tests;
