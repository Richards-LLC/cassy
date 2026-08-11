use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

use crate::TypeError;

/// Immutable worker-supplied completion evidence. Every identity-bearing
/// field is revalidated against registered CAS and live Git state before the
/// receipt is persisted.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerCompletionReceiptInput {
    pub task_id: String,
    pub worker_agent_id: String,
    pub repo_selector: String,
    pub source_branch: String,
    pub commit_sha: String,
    pub merge_base_sha: String,
    pub target_branch: String,
    pub target_sha: String,
    pub proof_reference: String,
    pub scope_summary: String,
    /// Optional durable proof artifact stored under the configured factory
    /// artifacts root. The close boundary validates its location before this
    /// immutable receipt is accepted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerCompletionReceipt {
    pub id: String,
    pub task_id: String,
    pub worker_agent_id: String,
    pub worker_name: String,
    pub repo_selector: String,
    pub source_branch: String,
    pub commit_sha: String,
    pub merge_base_sha: String,
    pub target_branch: String,
    pub target_sha: String,
    pub proof_reference: String,
    pub scope_summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub artifact_path: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkerDeliveryState {
    AwaitingVerification,
    AwaitingMerge,
    MergeAuthorized,
    Merged,
    CloseReady,
    Delivered,
    VerificationFailed,
    ChangesRequested,
    Conflict,
    Stale,
    RepoMismatch,
    TipChanged,
}

impl WorkerDeliveryState {
    pub fn is_recoverable_failure(self) -> bool {
        matches!(
            self,
            Self::VerificationFailed
                | Self::ChangesRequested
                | Self::Conflict
                | Self::Stale
                | Self::RepoMismatch
                | Self::TipChanged
        )
    }
}

impl fmt::Display for WorkerDeliveryState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::AwaitingVerification => "awaiting_verification",
            Self::AwaitingMerge => "awaiting_merge",
            Self::MergeAuthorized => "merge_authorized",
            Self::Merged => "merged",
            Self::CloseReady => "close_ready",
            Self::Delivered => "delivered",
            Self::VerificationFailed => "verification_failed",
            Self::ChangesRequested => "changes_requested",
            Self::Conflict => "conflict",
            Self::Stale => "stale",
            Self::RepoMismatch => "repo_mismatch",
            Self::TipChanged => "tip_changed",
        })
    }
}

impl FromStr for WorkerDeliveryState {
    type Err = TypeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "awaiting_verification" => Ok(Self::AwaitingVerification),
            "awaiting_merge" => Ok(Self::AwaitingMerge),
            "merge_authorized" => Ok(Self::MergeAuthorized),
            "merged" => Ok(Self::Merged),
            "close_ready" => Ok(Self::CloseReady),
            "delivered" => Ok(Self::Delivered),
            "verification_failed" => Ok(Self::VerificationFailed),
            "changes_requested" => Ok(Self::ChangesRequested),
            "conflict" => Ok(Self::Conflict),
            "stale" => Ok(Self::Stale),
            "repo_mismatch" => Ok(Self::RepoMismatch),
            "tip_changed" => Ok(Self::TipChanged),
            other => Err(TypeError::Parse(format!(
                "invalid worker delivery state `{other}`"
            ))),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerDeliveryTransaction {
    pub id: String,
    pub receipt_id: String,
    pub task_id: String,
    pub state: WorkerDeliveryState,
    pub supervisor_agent_id: Option<String>,
    pub verification_id: Option<String>,
    pub merge_commit_sha: Option<String>,
    pub last_error_code: Option<String>,
    pub last_error_detail: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerDeliveryEvent {
    pub id: String,
    pub transaction_id: String,
    pub state: WorkerDeliveryState,
    pub actor_agent_id: String,
    pub detail: Option<String>,
    pub created_at: DateTime<Utc>,
}
