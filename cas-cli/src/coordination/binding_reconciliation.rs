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
        ensure(
            bounded_text(team_id, 256),
            "team identity is empty, overlong or contains NUL",
        )?;
        ensure(
            bounded_text(canonical_project_id, 512),
            "canonical project identity is empty, overlong or contains NUL",
        )?;
        ensure(
            slack_id(channel_id, b"CG"),
            "channel must be a supported public/private Slack channel",
        )?;
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
        let (expected_status, code, action) = response.reason.contract();
        ensure(
            status == expected_status && response.code == code && response.next_action == action,
            "HTTP status, code, reason and next action disagree",
        )?;
        response.validate_payload(self)?;
        if response.code == ReconciliationCode::Completed {
            response.validate_completion()?;
        } else {
            response.validate_refusal()?;
        }
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
    NoEligibleReaders,
    RateLimited,
    AuthorityUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReconciliationReason {
    Activated,
    AlreadyManaged,
    MalformedRequest,
    PayloadTooLarge,
    UnsupportedSchema,
    InvalidCredential,
    WriteRequired,
    MembershipRevoked,
    UnlinkedProject,
    BindingScopeConflict,
    MappingProjectMismatch,
    ChannelUnmapped,
    WorkspaceMismatch,
    NotMember,
    LegacyIntakeActive,
    BindingUnevidenced,
    NoEligibleReaders,
    CallerRateLimited,
    AuthorityUnconfigured,
    ProofUnavailable,
    ProofInvalid,
    AuthorizationChanged,
    /// A mutation may have committed. Retain the pause and retry the identical
    /// tuple; neither a timer nor a negative GET proves that no effect happened.
    ActivationOutcomeUnknown,
}

impl ReconciliationReason {
    pub(super) fn contract(self) -> (u16, ReconciliationCode, NextAction) {
        use NextAction as A;
        use ReconciliationCode as C;
        match self {
            Self::Activated | Self::AlreadyManaged => (200, C::Completed, A::None),
            Self::MalformedRequest => (400, C::InvalidRequest, A::FixRequest),
            Self::PayloadTooLarge => (413, C::InvalidRequest, A::FixRequest),
            Self::UnsupportedSchema => (400, C::UnsupportedVersion, A::UpgradeClient),
            Self::InvalidCredential => (401, C::Unauthorized, A::Authenticate),
            Self::WriteRequired => (403, C::Forbidden, A::Authenticate),
            Self::MembershipRevoked => (403, C::Forbidden, A::RestoreMembership),
            Self::UnlinkedProject => (404, C::UnknownProject, A::LinkTeamProject),
            Self::BindingScopeConflict => (409, C::BindingConflict, A::ExplicitRelink),
            Self::MappingProjectMismatch => (409, C::MappingConflict, A::ExplicitRelink),
            Self::ChannelUnmapped => (409, C::MappingNotFound, A::OperatorRepair),
            Self::WorkspaceMismatch => (409, C::InstallationMismatch, A::OperatorRepair),
            Self::NotMember => (409, C::ChannelUnavailable, A::InviteViolet),
            Self::LegacyIntakeActive => (409, C::LegacyIntakeActive, A::StopLegacyIntake),
            Self::BindingUnevidenced => (409, C::BindingUnevidenced, A::OperatorRepair),
            Self::NoEligibleReaders => (409, C::NoEligibleReaders, A::RetryLater),
            Self::CallerRateLimited => (429, C::RateLimited, A::RetryLater),
            Self::AuthorityUnconfigured | Self::ProofInvalid => {
                (503, C::AuthorityUnavailable, A::OperatorRepair)
            }
            Self::ProofUnavailable
            | Self::AuthorizationChanged
            | Self::ActivationOutcomeUnknown => (503, C::AuthorityUnavailable, A::RetryLater),
        }
    }
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
    pub reason: ReconciliationReason,
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
    fn validate_payload(&self, target: &ReconciliationRequest) -> Result<(), WireError> {
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
            // Unlike registration-scoped GET, this response pins the invocation's
            // receipt even if a competing attempt finishes before evidence is read.
            let latest = self
                .latest_attempt
                .as_ref()
                .ok_or(WireError("attempt has no receipt"))?;
            let settled = self
                .settled
                .as_ref()
                .ok_or(WireError("attempt has no settlement"))?;
            ensure(id == &latest.id, "attempt identity disagrees with receipt")?;
            ensure(
                latest.quiescence.attested_v1_consumers.is_empty(),
                "ordinary invocation cannot exempt consumers",
            )?;
            ensure(
                settled.migrated == latest.settled.migrated_v1_rows
                    && settled.retired == latest.settled.retired_v1_rows
                    && settled.invalidated_leases == latest.settled.invalidated_v1_leases,
                "attempt settlement disagrees with receipt",
            )?;
        }
        if let Some(settled) = &self.settled {
            safe_numbers(&[
                settled.migrated,
                settled.retired,
                settled.invalidated_leases,
            ])?;
        }
        if let Some(retry) = self.retry_after_secs {
            safe_numbers(&[retry])?;
        }
        if let Some(blockers) = &self.blockers {
            safe_numbers(&[
                blockers.unexpired_v1_leases,
                blockers.live_v1_consumers_count,
            ])?;
            let shown = blockers.live_v1_consumers.len() as u64;
            ensure(
                shown <= 1000
                    && shown <= blockers.live_v1_consumers_count
                    && blockers.consumers_truncated == (shown < blockers.live_v1_consumers_count)
                    && blockers
                        .live_v1_consumers
                        .iter()
                        .all(|label| bounded_text(label, 200)),
                "blocker labels or truncation count are inconsistent",
            )?;
        }
        Ok(())
    }

    fn validate_refusal(&self) -> Result<(), WireError> {
        if self.reason == ReconciliationReason::NoEligibleReaders {
            let binding = self.binding.as_ref().ok_or(WireError("reader refusal has no binding"))?;
            let latest = self.latest_attempt.as_ref().ok_or(WireError("reader refusal has no attempted receipt"))?;
            ensure(
                self.activation_id.as_ref() == Some(&latest.id)
                    && latest.outcome == ActivationOutcome::Refused
                    && latest.fence_generation == binding.fence_generation
                    && self.blockers.is_none()
                    && self.retry_after_secs.is_none()
                    && self.settled.as_ref().is_some_and(|settled| {
                        settled.migrated == 0 && settled.retired == 0 && settled.invalidated_leases == 0
                    }),
                "reader refusal lacks unchanged attempt and settlement",
            )?;
            match binding.mode {
                BindingMode::Legacy => ensure(
                    self.strict_activation == Some(StrictActivation::Refused)
                        && self.fence_receipt.is_none()
                        && binding.fence_generation == 0
                        && binding.activated_at.is_none()
                        && binding.legacy_fenced_at.is_none()
                        && latest.prior_mode == BindingMode::Legacy,
                    "legacy reader refusal cannot carry fence evidence",
                )?,
                BindingMode::Managed => {
                    let fence = self.fence_receipt.as_ref().ok_or(WireError("managed reader refusal lost original fence"))?;
                    ensure(
                        self.strict_activation == Some(StrictActivation::Unevidenced)
                            && binding.verified
                            && binding.activated_at.is_some()
                            && binding.legacy_fenced_at.is_some()
                            && latest.prior_mode == BindingMode::Managed
                            && fence.outcome == ActivationOutcome::Activated
                            && fence.prior_mode != BindingMode::Managed
                            && fence.fence_generation == binding.fence_generation,
                        "managed reader refusal changed original fence",
                    )?;
                }
                BindingMode::Activating => return Err(WireError("reader refusal has activating binding")),
            }
        }
        if self.reason == ReconciliationReason::LegacyIntakeActive {
            let binding = self
                .binding
                .as_ref()
                .ok_or(WireError("legacy refusal has no binding"))?;
            let latest = self
                .latest_attempt
                .as_ref()
                .ok_or(WireError("legacy refusal has no attempted receipt"))?;
            ensure(
                self.blockers.is_some()
                    && self.activation_id.as_ref() == Some(&latest.id)
                    && latest.outcome == ActivationOutcome::Refused
                    && latest.fence_generation == binding.fence_generation
                    && binding.mode != BindingMode::Managed,
                "legacy refusal lacks matching attempt and blockers",
            )?;
        }
        if self.reason == ReconciliationReason::CallerRateLimited {
            ensure(
                self.retry_after_secs.is_some(),
                "rate limit has no retry hint",
            )?;
        }
        Ok(())
    }

    fn validate_completion(&self) -> Result<(), WireError> {
        ensure(
            self.strict_activation == Some(StrictActivation::Evidenced)
                && self.activation_id.is_some()
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
        ensure(
            binding.mode == BindingMode::Managed
                && binding.verified
                && binding.activated_at.is_some()
                && binding.legacy_fenced_at.is_some(),
            "completion binding is not verified and fenced",
        )?;
        ensure(
            fence.outcome == ActivationOutcome::Activated
                && fence.prior_mode != BindingMode::Managed
                && fence.quiescence.blocking_v1_leases == 0
                && fence.quiescence.blocking_v1_consumers.is_empty()
                && fence.fence_generation == binding.fence_generation
                && latest.fence_generation == binding.fence_generation,
            "receipt does not prove the current fence",
        )?;
        // A historical ADMIN fence may retain its original attested labels.
        // New ordinary attempts were checked above; never rewrite old evidence.
        match self.reason {
            ReconciliationReason::Activated => ensure(
                latest == fence,
                "new activation does not identify its original fence",
            ),
            ReconciliationReason::AlreadyManaged => ensure(
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
    // `use_z` only abbreviates zero offsets; it does not reject other zones.
    value.ends_with('Z')
        && chrono::DateTime::parse_from_rfc3339(value).is_ok_and(|time| {
            time.to_rfc3339_opts(SecondsFormat::Millis, true) == value
                || (legacy_receipt && time.to_rfc3339_opts(SecondsFormat::Secs, true) == value)
        })
}

#[cfg(test)]
mod tests;
