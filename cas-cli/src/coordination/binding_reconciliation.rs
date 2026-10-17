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
        status: u16,
        body: &[u8],
    ) -> Result<ReconciliationResponse, WireError> {
        let response: ReconciliationResponse = serde_json::from_slice(body)
            .map_err(|_| WireError("response does not match the strict versioned shape"))?;
        ensure(response.schema == SCHEMA, "unsupported response schema")?;
        ensure(
            status == 200 && response.code == ReconciliationCode::Completed,
            "unsupported status or refusal",
        )?;
        response.validate_scope(self)?;
        response.validate_completion()?;
        Ok(response)
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

impl ReconciliationResponse {
    fn validate_scope(&self, target: &ReconciliationRequest) -> Result<(), WireError> {
        if let Some(scope) = &self.scope {
            ensure(
                scope.team_id == target.team_id
                    && scope.project_id == target.project_id
                    && scope.channel_id == target.channel_id,
                "response belongs to a different target",
            )?;
            ensure(
                slack_id(&scope.workspace_id, b"T"),
                "invalid enrolled workspace",
            )?;
        }
        if let Some(binding) = &self.binding {
            let scope = self
                .scope
                .as_ref()
                .ok_or(WireError("binding has no authorized scope"))?;
            ensure(
                binding.team_id == scope.team_id
                    && binding.project_id == scope.project_id
                    && binding.workspace_id == scope.workspace_id
                    && binding.channel_id == scope.channel_id,
                "binding disagrees with authorized scope",
            )?;
            ensure(
                uuid(&binding.id) && binding.provider == "slack",
                "invalid binding identity",
            )?;
            safe_numbers(&[binding.fence_generation])?;
            for timestamp in [&binding.activated_at, &binding.legacy_fenced_at]
                .into_iter()
                .flatten()
            {
                ensure(utc_timestamp(timestamp, false), "invalid binding timestamp")?;
            }
        }
        for receipt in [&self.fence_receipt, &self.latest_attempt]
            .into_iter()
            .flatten()
        {
            receipt.validate()?;
        }
        if let Some(id) = &self.activation_id {
            ensure(uuid(id), "invalid activation identity")?;
        }
        if let Some(settled) = &self.settled {
            safe_numbers(&[
                settled.migrated,
                settled.retired,
                settled.invalidated_leases,
            ])?;
        }
        Ok(())
    }

    fn validate_completion(&self) -> Result<(), WireError> {
        ensure(
            self.strict_activation == Some(StrictActivation::Evidenced)
                && self.next_action == NextAction::None
                && self.blockers.is_none()
                && self.retry_after_secs.is_none(),
            "completion lacks strict evidenced state",
        )?;
        let binding = self
            .binding
            .as_ref()
            .ok_or(WireError("completion has no binding"))?;
        let fence = self
            .fence_receipt
            .as_ref()
            .ok_or(WireError("completion has no original fence receipt"))?;
        let latest = self
            .latest_attempt
            .as_ref()
            .ok_or(WireError("completion has no attempted receipt"))?;
        let settled = self
            .settled
            .as_ref()
            .ok_or(WireError("completion has no settlement"))?;
        ensure(
            binding.mode == BindingMode::Managed
                && binding.verified
                && binding.activated_at.is_some()
                && binding.legacy_fenced_at.is_some(),
            "completion binding is not verified and fenced",
        )?;
        ensure(
            fence.outcome == ActivationOutcome::Activated
                && fence.fence_generation == binding.fence_generation
                && latest.fence_generation == binding.fence_generation,
            "receipt does not prove the current fence",
        )?;
        ensure(
            self.activation_id.as_ref() == Some(&latest.id),
            "attempt identity disagrees with receipt",
        )?;
        ensure(
            fence.quiescence.attested_v1_consumers.is_empty()
                && latest.quiescence.attested_v1_consumers.is_empty(),
            "ordinary repair cannot exempt consumers",
        )?;
        ensure(
            settled.migrated == latest.settled.migrated_v1_rows
                && settled.retired == latest.settled.retired_v1_rows
                && settled.invalidated_leases == latest.settled.invalidated_v1_leases,
            "attempt settlement disagrees with receipt",
        )?;
        match self.reason.as_str() {
            "activated" => ensure(
                latest == fence,
                "new activation does not identify its original fence",
            ),
            "already_managed" => ensure(
                latest.outcome == ActivationOutcome::AlreadyManaged
                    && latest.prior_mode == BindingMode::Managed,
                "repeat does not identify a managed attempt",
            ),
            _ => Err(WireError("unsupported completion reason")),
        }
    }
}

impl ActivationReceipt {
    fn validate(&self) -> Result<(), WireError> {
        ensure(
            uuid(&self.id) && utc_timestamp(&self.attempted_at, true),
            "invalid receipt identity or timestamp",
        )?;
        let quiet = &self.quiescence;
        ensure(
            (1..=86400).contains(&quiet.quiet_window_secs),
            "invalid receipt quiet window",
        )?;
        ensure(
            quiet.attested_v1_consumers.len() <= 50
                && quiet.blocking_v1_consumers.len() <= 1000
                && quiet
                    .attested_v1_consumers
                    .iter()
                    .chain(&quiet.blocking_v1_consumers)
                    .all(|s| bounded_text(s, 200)),
            "invalid receipt consumer labels",
        )?;
        safe_numbers(&[
            self.fence_generation,
            quiet.blocking_v1_leases,
            self.settled.migrated_v1_rows,
            self.settled.retired_v1_rows,
            self.settled.invalidated_v1_leases,
        ])
    }
}

fn ensure(condition: bool, detail: &'static str) -> Result<(), WireError> {
    if condition {
        Ok(())
    } else {
        Err(WireError(detail))
    }
}

fn safe_numbers(numbers: &[u64]) -> Result<(), WireError> {
    // Cloud's existing immutable receipt uses JavaScript safe integers.
    ensure(
        numbers.iter().all(|n| *n <= 9_007_199_254_740_991),
        "count or generation exceeds wire integer range",
    )
}

fn bounded_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && !value.contains('\0') && value.encode_utf16().count() <= max
}

fn slack_id(value: &str, prefixes: &[u8]) -> bool {
    let bytes = value.as_bytes();
    (3..=31).contains(&bytes.len())
        && prefixes.contains(&bytes[0])
        && bytes[1..]
            .iter()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

fn uuid(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok_and(|id| id.to_string() == value)
}

fn utc_timestamp(value: &str, legacy_receipt: bool) -> bool {
    use chrono::SecondsFormat;
    chrono::DateTime::parse_from_rfc3339(value).is_ok_and(|time| {
        time.to_rfc3339_opts(SecondsFormat::Millis, true) == value
            || (legacy_receipt && time.to_rfc3339_opts(SecondsFormat::Secs, true) == value)
    })
}

#[cfg(test)]
mod tests;
