//! Prompt queue for supervisor → worker communication in factory sessions
//!
//! Allows supervisor agents to send prompts to workers via MCP.
//! Factory TUI polls this queue and injects prompts into worker PTYs.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::{Result, StoreError};
use crate::recording_store::capture_message_event;
use crate::supervisor_queue_store::NotificationPriority;

/// Retry policy for daemon-owned prompt delivery.
///
/// The delay is exponential (250ms → 5s cap), while either 120 failed
/// attempts or fifteen minutes since the first failed attempt makes the row
/// terminal. Queueing age is deliberately excluded: a target may legitimately
/// remain unregistered for longer than the retry window without any delivery
/// having been attempted. Keeping the policy in the store ensures every daemon
/// caller applies the same bound.
pub const PROMPT_RETRY_MAX_ATTEMPTS: u32 = 120;
pub const PROMPT_RETRY_MAX_AGE_SECS: i64 = 15 * 60;
const PROMPT_RETRY_BASE_DELAY_MS: i64 = 250;
const PROMPT_RETRY_MAX_DELAY_MS: i64 = 5_000;
/// Exact-content worker reports are collapsed only while a recent,
/// transport-delivered copy is still awaiting confirmation.
const PROMPT_DUPLICATE_WINDOW_SECS: i64 = 30;

/// Result of recording a failed daemon delivery attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PromptRetryDisposition {
    /// The row remains pending but is ineligible until `retry_at`.
    Scheduled {
        attempts: u32,
        retry_at: DateTime<Utc>,
    },
    /// The bounded retry/age policy terminally abandoned the row.
    Abandoned { attempts: u32 },
}

/// A prompt in the queue
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedPrompt {
    /// Unique prompt ID
    pub id: i64,
    /// Source agent (who sent the prompt)
    pub source: String,
    /// Target agent name or "all_workers"
    pub target: String,
    /// The prompt text to inject
    pub prompt: String,
    /// When the prompt was queued
    pub created_at: DateTime<Utc>,
    /// When the prompt was processed (None if pending)
    pub processed_at: Option<DateTime<Utc>>,
    /// Owning factory session for session-scoped delivery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub factory_session: Option<String>,
    /// Short summary for UI display
    pub summary: Option<String>,
    /// Message priority (lower = higher priority)
    pub priority: NotificationPriority,
    /// When the target agent acknowledged receipt (None if not yet acked)
    pub acked_at: Option<DateTime<Utc>>,
    /// Urgent delivery flag (cas-c931): when true, the daemon breaks the
    /// target's in-flight turn (Esc) and injects via the PTY, bypassing the
    /// Claude Code inbox even in agent-teams mode. Default false = normal
    /// inbox/queue delivery (non-disruptive).
    pub urgent: bool,
}

/// Schema for prompt queue table
const PROMPT_QUEUE_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS prompt_queue (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    source TEXT NOT NULL,
    target TEXT NOT NULL,
    prompt TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    processed_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_prompt_queue_pending ON prompt_queue(target) WHERE processed_at IS NULL;
"#;

/// Add factory_session column for multi-session isolation.
/// Uses IF NOT EXISTS via a safe column-add pattern.
const PROMPT_QUEUE_SESSION_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN factory_session TEXT;
"#;

/// Add summary column for UI display.
const PROMPT_QUEUE_SUMMARY_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN summary TEXT;
"#;

/// Add priority column for message ordering (0=Critical, 1=High, 2=Normal).
const PROMPT_QUEUE_PRIORITY_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN priority INTEGER NOT NULL DEFAULT 2;
"#;

/// Add acked_at column for delivery confirmation.
const PROMPT_QUEUE_ACKED_AT_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN acked_at TEXT;
"#;

/// Add urgent column (cas-c931) for interrupt-and-redirect delivery.
/// 0 = normal inbox/queue delivery (default), 1 = break the target's turn
/// (Esc) then inject via PTY, bypassing the Claude Code inbox.
const PROMPT_QUEUE_URGENT_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN urgent INTEGER NOT NULL DEFAULT 0;
"#;

/// Indexes supporting two-lane `peek_for_targets` selection (cas-2bcb).
/// Partial indexes keep the path bounded to pending rows only.
const PROMPT_QUEUE_TWO_LANE_INDEXES: &str = r#"
CREATE INDEX IF NOT EXISTS idx_prompt_queue_session_pending
    ON prompt_queue(factory_session, priority, id)
    WHERE processed_at IS NULL AND factory_session IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_prompt_queue_legacy_pending
    ON prompt_queue(target, priority, id)
    WHERE processed_at IS NULL AND factory_session IS NULL;
"#;

/// Idempotent lifecycle outbox enqueue key (cas-ecff).
const PROMPT_QUEUE_DEDUPE_KEY_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN dedupe_key TEXT;
"#;
const PROMPT_QUEUE_DEDUPE_KEY_INDEX: &str = r#"
CREATE UNIQUE INDEX IF NOT EXISTS idx_prompt_queue_dedupe_key
    ON prompt_queue(dedupe_key)
    WHERE dedupe_key IS NOT NULL;
"#;

/// Indexes for the two per-send queue queries (cas-c061).
///
/// These deliberately live outside [`PROMPT_QUEUE_SCHEMA`]: `acked_at`,
/// `urgent`, `factory_session`, and `transport_delivered_at` are migration-
/// added columns and do not exist when the baseline schema is applied over a
/// legacy table. `init` installs these only after every column migration.
const PROMPT_QUEUE_MESSAGE_HOT_PATH_INDEXES_MIGRATION: &str = r#"
CREATE INDEX IF NOT EXISTS idx_prompt_queue_recent_unacked_dedupe
    ON prompt_queue(
        source,
        target,
        prompt,
        factory_session,
        transport_delivered_at DESC,
        id DESC
    )
    WHERE urgent = 0
      AND transport_delivered_at IS NOT NULL
      AND acked_at IS NULL
      AND highest_stage IS NOT 'confirmed';
CREATE INDEX IF NOT EXISTS idx_prompt_queue_ack_counterparty
    ON prompt_queue(target, source, factory_session)
    WHERE transport_delivered_at IS NOT NULL
      AND acked_at IS NULL;
"#;

/// Result of a normal queue enqueue that may collapse a recent worker resend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// New row inserted.
    Created(i64),
    /// A recent, delivered, still-unconfirmed identical worker report exists.
    SuppressedDuplicate(i64),
}

impl EnqueueOutcome {
    /// Queue row ID created or reused by this enqueue.
    pub fn id(self) -> i64 {
        match self {
            Self::Created(id) | Self::SuppressedDuplicate(id) => id,
        }
    }
}

/// Result of an idempotent prompt enqueue (cas-ecff).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueIdempotentResult {
    /// New row inserted.
    Created(i64),
    /// Existing row for the same dedupe_key (no second insert).
    AlreadyExists(i64),
}

/// Delivery status of a prompt queue message (legacy three-value ladder).
///
/// Preserved for existing MCP/clients. Prefer [`MessageDeliveryReport`] for
/// stage-based observability (cas-2c5f).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum MessageStatus {
    /// Message is queued but not yet delivered
    Pending,
    /// Message was injected/delivered but not yet acknowledged by the target
    Delivered,
    /// Target agent has confirmed receipt
    Confirmed,
}

impl std::fmt::Display for MessageStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Delivered => write!(f, "delivered"),
            Self::Confirmed => write!(f, "confirmed"),
        }
    }
}

/// Monotonic transport stage CAS can authoritatively observe (cas-2c5f).
///
/// Rank order:
/// Enqueued < Selected < Gated <
///   {Dropped|Suppressed|Abandoned|PartiallyDelivered|Delivered} < Confirmed
///
/// **Delivered** requires successful handoff to *all* intended recipients and
/// `transport_delivered_at`. Partial `all_workers` success is
/// [`DeliveryStage::PartiallyDelivered`] (never silent full Delivered).
///
/// Terminal siblings share rank 3; only the legal same-rank transition is
/// `PartiallyDelivered → Delivered`. Illegal sibling rewrites are rejected.
/// Corrupt/unknown `highest_stage` is a typed error (never silent Enqueued).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeliveryStage {
    /// Row exists with parseable `created_at`.
    Enqueued,
    /// Daemon selected/peeked the row for a delivery attempt.
    Selected,
    /// Delivery attempt blocked by a gate (pane not ready, target unavailable).
    Gated,
    /// Intentionally not delivered: dead-source drop.
    Dropped,
    /// Intentionally not delivered: idle-message rate-limit suppression.
    Suppressed,
    /// Intentionally not delivered: unknown/stale target abandoned.
    Abandoned,
    /// Broadcast reached some but not all intended recipients.
    PartiallyDelivered,
    /// Authoritative full transport handoff (all intended recipients Ok).
    Delivered,
    /// Target acknowledged (`acked_at` set).
    Confirmed,
}

impl DeliveryStage {
    pub fn rank(self) -> u8 {
        match self {
            Self::Enqueued => 0,
            Self::Selected => 1,
            Self::Gated => 2,
            Self::Dropped
            | Self::Suppressed
            | Self::Abandoned
            | Self::PartiallyDelivered
            | Self::Delivered => 3,
            Self::Confirmed => 4,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Enqueued => "enqueued",
            Self::Selected => "selected",
            Self::Gated => "gated",
            Self::Dropped => "dropped",
            Self::Suppressed => "suppressed",
            Self::Abandoned => "abandoned",
            Self::PartiallyDelivered => "partially_delivered",
            Self::Delivered => "delivered",
            Self::Confirmed => "confirmed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "enqueued" => Some(Self::Enqueued),
            "selected" => Some(Self::Selected),
            "gated" => Some(Self::Gated),
            "dropped" => Some(Self::Dropped),
            "suppressed" => Some(Self::Suppressed),
            "abandoned" => Some(Self::Abandoned),
            "partially_delivered" => Some(Self::PartiallyDelivered),
            "delivered" => Some(Self::Delivered),
            "confirmed" => Some(Self::Confirmed),
            _ => None,
        }
    }

    pub fn is_terminal_non_delivery(self) -> bool {
        matches!(self, Self::Dropped | Self::Suppressed | Self::Abandoned)
    }

    /// Whether transition `self → to` is legal (idempotent same stage always ok).
    pub fn can_transition_to(self, to: Self) -> bool {
        if self == to {
            return true;
        }
        if to.rank() > self.rank() {
            return true;
        }
        // Sole legal same-rank rewrite: partial → full delivery (retry completed).
        self == Self::PartiallyDelivered && to == Self::Delivered
    }
}

impl std::fmt::Display for DeliveryStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Why a message has not completed transport confirmation (cas-2c5f).
///
/// Only reasons CAS can authoritatively stamp or derive without false precision.
/// Inaccurate queue-head inference was removed (review reject of ae8f47b).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PendingReason {
    /// Row is not eligible for the observing session's selection rules.
    SessionIneligible,
    /// Target agent/pane is not available to receive delivery.
    TargetUnavailable,
    /// Delivery gated (e.g. pane not ready for injection).
    GatedNotReady,
    /// Last adapter attempt failed; row left pending for retry.
    AdapterRetryable,
    /// Enqueued/selected and not known blocked; awaiting next delivery tick.
    AwaitingDelivery,
    /// Authoritative transport delivered; waiting for target `message_ack`.
    AwaitingAck,
    /// Terminal non-delivery: dead worker source dropped.
    DroppedDeadSource,
    /// Terminal non-delivery: duplicate idle suppression.
    SuppressedIdle,
    /// Terminal non-delivery: unknown/stale target abandoned.
    AbandonedUnknownTarget,
    /// Broadcast reached a subset of intended recipients.
    PartialBroadcast,
    /// Broadcast had zero intended recipients (e.g. no non-native workers).
    NoIntendedRecipients,
}

impl PendingReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SessionIneligible => "session_ineligible",
            Self::TargetUnavailable => "target_unavailable",
            Self::GatedNotReady => "gated_not_ready",
            Self::AdapterRetryable => "adapter_retryable",
            Self::AwaitingDelivery => "awaiting_delivery",
            Self::AwaitingAck => "awaiting_ack",
            Self::DroppedDeadSource => "dropped_dead_source",
            Self::SuppressedIdle => "suppressed_idle",
            Self::AbandonedUnknownTarget => "abandoned_unknown_target",
            Self::PartialBroadcast => "partial_broadcast",
            Self::NoIntendedRecipients => "no_intended_recipients",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "session_ineligible" => Some(Self::SessionIneligible),
            "target_unavailable" => Some(Self::TargetUnavailable),
            "gated_not_ready" => Some(Self::GatedNotReady),
            "adapter_retryable" => Some(Self::AdapterRetryable),
            "awaiting_delivery" => Some(Self::AwaitingDelivery),
            "awaiting_ack" => Some(Self::AwaitingAck),
            "dropped_dead_source" => Some(Self::DroppedDeadSource),
            "suppressed_idle" => Some(Self::SuppressedIdle),
            "abandoned_unknown_target" => Some(Self::AbandonedUnknownTarget),
            "partial_broadcast" => Some(Self::PartialBroadcast),
            "no_intended_recipients" => Some(Self::NoIntendedRecipients),
            "behind_queue_head" => Some(Self::AwaitingDelivery),
            _ => None,
        }
    }

    fn implied_stage(self) -> DeliveryStage {
        match self {
            Self::GatedNotReady | Self::TargetUnavailable => DeliveryStage::Gated,
            Self::DroppedDeadSource => DeliveryStage::Dropped,
            Self::SuppressedIdle => DeliveryStage::Suppressed,
            Self::AbandonedUnknownTarget => DeliveryStage::Abandoned,
            Self::PartialBroadcast => DeliveryStage::PartiallyDelivered,
            Self::NoIntendedRecipients => DeliveryStage::Selected,
            Self::AdapterRetryable
            | Self::AwaitingDelivery
            | Self::SessionIneligible
            | Self::AwaitingAck => DeliveryStage::Selected,
        }
    }
}

impl std::fmt::Display for PendingReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Evidence CAS does **not** fabricate (harness wake / model reaction).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStatus {
    /// No authoritative CAS observation for this stage.
    #[default]
    Unobserved,
}

impl std::fmt::Display for ObservationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unobserved => write!(f, "unobserved"),
        }
    }
}

/// Stage-based delivery report for one prompt_queue message (cas-2c5f).
///
/// Additive to legacy [`MessageStatus`]. Wake/reaction are always
/// [`ObservationStatus::Unobserved`] — never inferred from timestamps.
///
/// `delivered_at` is **only** set after full transport handoff
/// (`transport_delivered_at`). Partial broadcasts never populate it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageDeliveryReport {
    pub id: i64,
    /// Legacy three-value status (processed_at/acked_at) for older clients.
    pub legacy_status: MessageStatus,
    /// Highest monotonic stage reached (authoritative columns only).
    pub stage: DeliveryStage,
    pub source: String,
    pub target: String,
    pub factory_session: Option<String>,
    pub priority: u8,
    pub urgent: bool,
    pub enqueued_at: DateTime<Utc>,
    pub selected_at: Option<DateTime<Utc>>,
    /// Full transport handoff time only (not legacy processed_at / partial).
    pub delivered_at: Option<DateTime<Utc>>,
    pub confirmed_at: Option<DateTime<Utc>>,
    /// Present when waiting/blocked, partial, or terminal non-delivery.
    pub pending_reason: Option<PendingReason>,
    /// Human-readable detail for the pending reason (error text, …).
    pub pending_detail: Option<String>,
    /// Broadcast recipient counts when applicable (`all_workers`).
    pub broadcast_attempted: Option<u32>,
    pub broadcast_succeeded: Option<u32>,
    pub broadcast_failed: Option<u32>,
    pub wake: ObservationStatus,
    pub reaction: ObservationStatus,
}

/// Stage-observability columns (cas-2c5f). Idempotent ALTERs.
const PROMPT_QUEUE_SELECTED_AT_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN selected_at TEXT;
"#;
const PROMPT_QUEUE_PENDING_REASON_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN last_pending_reason TEXT;
"#;
const PROMPT_QUEUE_PENDING_DETAIL_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN last_pending_detail TEXT;
"#;
const PROMPT_QUEUE_TRANSPORT_DELIVERED_AT_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN transport_delivered_at TEXT;
"#;
const PROMPT_QUEUE_HIGHEST_STAGE_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN highest_stage TEXT;
"#;
const PROMPT_QUEUE_BROADCAST_ATTEMPTED_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN broadcast_attempted INTEGER;
"#;
const PROMPT_QUEUE_BROADCAST_SUCCEEDED_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN broadcast_succeeded INTEGER;
"#;
const PROMPT_QUEUE_BROADCAST_FAILED_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN broadcast_failed INTEGER;
"#;
const PROMPT_QUEUE_DELIVERY_ATTEMPTS_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN delivery_attempts INTEGER NOT NULL DEFAULT 0;
"#;
const PROMPT_QUEUE_NEXT_ATTEMPT_AT_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN next_attempt_at TEXT;
"#;
const PROMPT_QUEUE_FIRST_ATTEMPT_AT_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN first_attempt_at TEXT;
"#;

/// Trait for prompt queue operations
pub trait PromptQueueStore: Send + Sync {
    /// Initialize the store (create tables)
    fn init(&self) -> Result<()>;

    /// Queue a prompt for a target agent
    fn enqueue(&self, source: &str, target: &str, prompt: &str) -> Result<i64>;

    /// Queue a prompt tagged with a factory session for isolation
    fn enqueue_with_session(
        &self,
        source: &str,
        target: &str,
        prompt: &str,
        factory_session: &str,
    ) -> Result<i64>;

    /// Queue a prompt with session, summary, and priority for UI display
    fn enqueue_with_summary(
        &self,
        source: &str,
        target: &str,
        prompt: &str,
        factory_session: Option<&str>,
        summary: Option<&str>,
    ) -> Result<i64> {
        self.enqueue_full(source, target, prompt, factory_session, summary, None)
    }

    /// Queue a prompt with all options including priority.
    ///
    /// Equivalent to [`PromptQueueStore::enqueue_urgent`] with `urgent = false`.
    fn enqueue_full(
        &self,
        source: &str,
        target: &str,
        prompt: &str,
        factory_session: Option<&str>,
        summary: Option<&str>,
        priority: Option<NotificationPriority>,
    ) -> Result<i64> {
        self.enqueue_urgent(
            source,
            target,
            prompt,
            factory_session,
            summary,
            priority,
            false,
        )
    }

    /// Queue a prompt with all options, including the cas-c931 `urgent` flag.
    ///
    /// When `urgent` is true, the daemon delivers via interrupt-and-redirect:
    /// it breaks the target worker's in-flight turn (Esc) and injects the
    /// message via the PTY, bypassing the Claude Code inbox even in agent-teams
    /// mode. When false, delivery is unchanged (inbox/queue, non-disruptive).
    fn enqueue_urgent(
        &self,
        source: &str,
        target: &str,
        prompt: &str,
        factory_session: Option<&str>,
        summary: Option<&str>,
        priority: Option<NotificationPriority>,
        urgent: bool,
    ) -> Result<i64> {
        Ok(self
            .enqueue_urgent_with_outcome(
                source,
                target,
                prompt,
                factory_session,
                summary,
                priority,
                urgent,
            )?
            .id())
    }

    /// Queue with an observable result for recent exact-content suppression.
    ///
    /// Legacy callers can continue using [`PromptQueueStore::enqueue_urgent`]
    /// when they only need the row ID. User-facing send paths should use this
    /// method so a reused ID is never presented as a fresh enqueue.
    fn enqueue_urgent_with_outcome(
        &self,
        source: &str,
        target: &str,
        prompt: &str,
        factory_session: Option<&str>,
        summary: Option<&str>,
        priority: Option<NotificationPriority>,
        urgent: bool,
    ) -> Result<EnqueueOutcome>;

    /// Idempotent enqueue keyed by `dedupe_key` (cas-ecff lifecycle outbox).
    ///
    /// Replaying the same key returns [`EnqueueIdempotentResult::AlreadyExists`]
    /// without inserting a second row — stamp-failure recovery cannot duplicate.
    fn enqueue_idempotent(
        &self,
        source: &str,
        target: &str,
        prompt: &str,
        factory_session: Option<&str>,
        summary: Option<&str>,
        priority: Option<NotificationPriority>,
        dedupe_key: &str,
    ) -> Result<EnqueueIdempotentResult>;

    /// Poll for pending prompts for a specific target (marks as processed)
    fn poll_for_target(&self, target: &str, limit: usize) -> Result<Vec<QueuedPrompt>>;

    /// Poll for pending prompts for a specific target within a factory session.
    ///
    /// `None` preserves legacy behavior. When a session is supplied, tagged
    /// rows only match that session, while NULL-session legacy rows still use
    /// the historical target/all_workers matching path.
    fn poll_for_target_with_session(
        &self,
        target: &str,
        factory_session: Option<&str>,
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>>;

    /// Poll all pending prompts (for Factory TUI to process)
    fn poll_all(&self, limit: usize) -> Result<Vec<QueuedPrompt>>;

    /// Peek at pending prompts without marking as processed
    fn peek_all(&self, limit: usize) -> Result<Vec<QueuedPrompt>>;

    /// Peek at pending prompts for specific targets only.
    ///
    /// # Eligibility (applied before LIMIT)
    /// - **Session lane:** rows with `factory_session` equal to the supplied
    ///   session (never matched by target-name collision alone).
    /// - **Legacy lane:** NULL-session rows whose `target` is in `targets`
    ///   (historical compatibility arm).
    ///
    /// # Cross-lane selection contract (when `factory_session` is set)
    /// Two independent peeks (each `ORDER BY priority ASC, id ASC LIMIT n`)
    /// are merged with **global priority first**, then same-priority lane
    /// fairness (cas-2bcb / cas-04a6 R1):
    /// 1. Walk priority bands from highest urgency (lowest numeric priority)
    ///    to lowest. No lower-priority row is selected while any higher-
    ///    priority eligible row remains in either lane's candidate set.
    /// 2. Within a single priority band, apply a bounded two-lane quota so
    ///    neither session nor legacy can permanently occupy the band's
    ///    remaining slots: `ceil(remaining/2)` session + `floor(remaining/2)`
    ///    legacy when both have work at that priority; unused quota fills
    ///    the other lane. Final order within a band: `id ASC` (FIFO).
    /// 3. `limit == 1`: the single slot is the highest-priority eligible
    ///    head; only when both heads share the same priority does equal-
    ///    priority fairness apply (session preferred at limit=1).
    ///
    /// Without a session tag, behavior is the historical single-lane target
    /// filter. Session isolation: other sessions' tagged rows never leak.
    ///
    /// # Errors
    /// Returns an error when `targets` is empty. Session-wide peeks are not
    /// supported; callers must state the exact delivery target universe.
    fn peek_for_targets(
        &self,
        targets: &[&str],
        factory_session: Option<&str>,
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>>;

    /// Return every outstanding prompt for specific targets.
    ///
    /// This is an observability query, not a delivery-selection query:
    /// scheduled retries remain outstanding and are therefore included even
    /// while `next_attempt_at` is in the future. Session isolation matches
    /// `peek_for_targets`: a supplied session sees its tagged rows plus
    /// legacy NULL-session rows; without a session only legacy rows are seen.
    fn outstanding_for_targets(
        &self,
        targets: &[&str],
        factory_session: Option<&str>,
    ) -> Result<Vec<QueuedPrompt>>;

    /// Most recent message timestamp for each target from any of `sources`.
    ///
    /// Unlike the pending-message APIs, this includes processed rows. The
    /// director uses it to remember that a supervisor has already handled a
    /// worker's current idle transition after the transport drains the row.
    fn latest_created_at_for_targets_from_sources(
        &self,
        sources: &[&str],
        targets: &[&str],
        factory_session: Option<&str>,
    ) -> Result<HashMap<String, DateTime<Utc>>>;

    /// Mark a prompt as processed
    fn mark_processed(&self, prompt_id: i64) -> Result<()>;

    /// Acknowledge receipt of a prompt (target agent confirms delivery)
    fn ack(&self, prompt_id: i64) -> Result<()>;

    /// Confirm delivered messages that the current recipient demonstrably
    /// consumed by sending a response back to one of `sender_aliases`.
    ///
    /// Factory supervisors have both a display name and the logical
    /// `"supervisor"` alias, so both sides are expressed as alias slices.
    /// Only transport-delivered, still-unacked rows in the observing factory
    /// session are advanced.
    fn ack_delivered_for_recipient(
        &self,
        recipient_aliases: &[&str],
        sender_aliases: &[&str],
        factory_session: Option<&str>,
    ) -> Result<usize>;

    /// Get messages that were processed but not acked within the timeout
    fn unacked(&self, timeout_secs: i64, limit: usize) -> Result<Vec<QueuedPrompt>>;

    /// Get delivery status of a specific message (legacy three-value ladder).
    fn message_status(&self, prompt_id: i64) -> Result<Option<MessageStatus>>;

    /// Stage-based delivery report (cas-2c5f). `None` if the id does not exist.
    /// Returns `Err` if required timestamps are corrupt (never fabricates `now`).
    fn message_delivery_report(&self, prompt_id: i64) -> Result<Option<MessageDeliveryReport>>;

    /// Record that the daemon selected/peeked this message for a delivery attempt.
    fn record_selected(&self, prompt_id: i64) -> Result<()>;

    /// Record a durable pending reason; advances highest_stage monotonically.
    fn record_pending_reason(
        &self,
        prompt_id: i64,
        reason: PendingReason,
        detail: Option<&str>,
    ) -> Result<()>;

    /// Record a failed daemon handoff with bounded exponential retry.
    ///
    /// Rows remain pending but are omitted from peeks until their retry time.
    /// Exhausted rows, or rows over-age since their first failed attempt,
    /// become terminal `Abandoned`.
    fn record_retry(
        &self,
        prompt_id: i64,
        reason: PendingReason,
        detail: Option<&str>,
    ) -> Result<PromptRetryDisposition>;

    /// Authoritative full transport handoff (all intended recipients Ok).
    /// Atomically sets `transport_delivered_at` + stage Delivered + `processed_at`.
    fn mark_transport_delivered(&self, prompt_id: i64) -> Result<()>;

    /// Broadcast outcome for `all_workers` (attempted/succeeded/failed counts).
    ///
    /// - all succeeded → Delivered + transport_delivered_at
    /// - mixed → PartiallyDelivered (no transport_delivered_at; processed to avoid re-inject)
    /// - zero succeeded (attempted > 0) → pending AdapterRetryable (not processed)
    /// - attempted == 0 → pending NoIntendedRecipients (not processed)
    fn mark_broadcast_outcome(
        &self,
        prompt_id: i64,
        attempted: u32,
        succeeded: u32,
        failed: u32,
        detail: Option<&str>,
    ) -> Result<()>;

    /// Dead-source drop: marks processed for queue drainage without transport success.
    fn mark_dropped(&self, prompt_id: i64, detail: Option<&str>) -> Result<()>;

    /// Idle-message suppression: processed without transport success.
    fn mark_suppressed(&self, prompt_id: i64, detail: Option<&str>) -> Result<()>;

    /// Unknown-target abandon: processed without transport success.
    fn mark_abandoned(&self, prompt_id: i64, detail: Option<&str>) -> Result<()>;

    /// Get count of pending prompts
    fn pending_count(&self) -> Result<usize>;

    /// Terminally abandon pending prompts older than the requested age.
    ///
    /// This is the safe remediation primitive for historical poison queues:
    /// it preserves rows and their forensic status instead of deleting them.
    fn abandon_pending_older_than(&self, older_than_secs: i64) -> Result<usize>;

    /// Abandon aged, session-scoped rows whose target is no longer a member
    /// of that session. Fresh rows stay pending so pre-registration delivery
    /// retains its grace period.
    fn abandon_ineligible_session_targets(
        &self,
        targets: &[&str],
        factory_session: &str,
        older_than_secs: i64,
    ) -> Result<usize>;

    /// Clear all prompts (for cleanup)
    fn clear(&self) -> Result<usize>;

    /// Clear old processed prompts (cleanup)
    fn cleanup_old(&self, older_than_secs: i64) -> Result<usize>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite-based prompt queue store
pub struct SqlitePromptQueueStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqlitePromptQueueStore {
    /// Open or create a SQLite prompt queue store
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let db_path = cas_dir.join("cas.db");
        let conn = crate::shared_db::shared_connection(&db_path)?;

        Ok(Self { conn })
    }

    fn parse_datetime(s: &str) -> Option<DateTime<Utc>> {
        if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
            return Some(dt.with_timezone(&Utc));
        }
        if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
            return Some(Utc.from_utc_datetime(&dt));
        }
        None
    }

    fn prompt_from_row(row: &rusqlite::Row) -> rusqlite::Result<QueuedPrompt> {
        let processed_at_str: Option<String> = row.get(5)?;
        let processed_at = processed_at_str.and_then(|s| Self::parse_datetime(&s));
        let summary: Option<String> = row.get(6).unwrap_or(None);
        let priority: u8 = row.get(7).unwrap_or(2);
        let acked_at_str: Option<String> = row.get(8).unwrap_or(None);
        let acked_at = acked_at_str.and_then(|s| Self::parse_datetime(&s));
        // Column 9 = urgent (cas-c931). Tolerate absence on legacy rows/tables.
        let urgent: bool = row.get::<_, i64>(9).map(|v| v != 0).unwrap_or(false);
        let factory_session: Option<String> = row.get(10).unwrap_or(None);

        Ok(QueuedPrompt {
            id: row.get(0)?,
            source: row.get(1)?,
            target: row.get(2)?,
            prompt: row.get(3)?,
            created_at: Self::parse_datetime(&row.get::<_, String>(4)?).unwrap_or_else(Utc::now),
            processed_at,
            factory_session,
            summary,
            priority: NotificationPriority::from(priority),
            acked_at,
            urgent,
        })
    }

    fn record_retry(
        &self,
        prompt_id: i64,
        reason: PendingReason,
        detail: Option<&str>,
    ) -> Result<PromptRetryDisposition> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            let row = tx
                .query_row(
                    "SELECT first_attempt_at, delivery_attempts
                     FROM prompt_queue
                     WHERE id = ? AND processed_at IS NULL",
                    params![prompt_id],
                    |row| Ok((row.get::<_, Option<String>>(0)?, row.get::<_, u32>(1)?)),
                )
                .optional()?;
            let Some((first_attempt_at_raw, attempts_before)) = row else {
                return Ok(PromptRetryDisposition::Abandoned { attempts: 0 });
            };

            let now = Utc::now();
            let attempts = attempts_before.saturating_add(1);
            let first_attempt_at = first_attempt_at_raw
                .as_deref()
                .map(Self::parse_datetime)
                .unwrap_or(Some(now));
            let age_secs = first_attempt_at
                .map(|first_attempt| (now - first_attempt).num_seconds().max(0))
                .unwrap_or(PROMPT_RETRY_MAX_AGE_SECS);
            let first_attempt_at_stamp = first_attempt_at_raw.unwrap_or_else(|| now.to_rfc3339());
            if attempts >= PROMPT_RETRY_MAX_ATTEMPTS || age_secs >= PROMPT_RETRY_MAX_AGE_SECS {
                let exhaustion = format!(
                    "{}; delivery abandoned after {attempts} failed attempts / {age_secs}s age",
                    detail.unwrap_or("delivery retry exhausted")
                );
                Self::atomic_stage_stamp_in_tx(
                    &tx,
                    prompt_id,
                    DeliveryStage::Abandoned,
                    AtomicStampOpts {
                        reason: Some(PendingReason::AbandonedUnknownTarget),
                        detail: Some(&exhaustion),
                        set_processed: true,
                        broadcast_attempted: None,
                        broadcast_succeeded: None,
                        broadcast_failed: None,
                    },
                )?;
                tx.execute(
                    "UPDATE prompt_queue
                     SET delivery_attempts = ?,
                         first_attempt_at = COALESCE(first_attempt_at, ?),
                         next_attempt_at = NULL
                     WHERE id = ?",
                    params![attempts, first_attempt_at_stamp, prompt_id],
                )?;
                tx.commit()?;
                return Ok(PromptRetryDisposition::Abandoned { attempts });
            }

            let exponent = attempts.saturating_sub(1).min(20);
            let delay_ms = PROMPT_RETRY_BASE_DELAY_MS
                .saturating_mul(1_i64 << exponent)
                .min(PROMPT_RETRY_MAX_DELAY_MS);
            let retry_at = now + chrono::Duration::milliseconds(delay_ms);
            Self::atomic_stage_stamp_in_tx(
                &tx,
                prompt_id,
                reason.implied_stage(),
                AtomicStampOpts::reason(reason, detail),
            )?;
            tx.execute(
                "UPDATE prompt_queue
                 SET delivery_attempts = ?,
                     first_attempt_at = COALESCE(first_attempt_at, ?),
                     next_attempt_at = ?
                 WHERE id = ?",
                params![
                    attempts,
                    first_attempt_at_stamp,
                    retry_at.to_rfc3339(),
                    prompt_id
                ],
            )?;
            tx.commit()?;
            Ok(PromptRetryDisposition::Scheduled { attempts, retry_at })
        })
    }

    /// Merge session-lane and legacy-lane peeks.
    ///
    /// Contract (see `PromptQueueStore::peek_for_targets`):
    /// 1. Global priority bands first (never emit priority P+1 while any
    ///    priority ≤P candidate remains).
    /// 2. Within one priority band only: bounded two-lane quota
    ///    (ceil(n/2) session + floor(n/2) legacy; unused fills the other).
    /// 3. Within a band, FIFO by id across the selected set.
    fn merge_two_lane_peeks(
        session: Vec<QueuedPrompt>,
        legacy: Vec<QueuedPrompt>,
        limit: usize,
    ) -> Vec<QueuedPrompt> {
        if limit == 0 {
            return Vec::new();
        }
        if session.is_empty() {
            return legacy.into_iter().take(limit).collect();
        }
        if legacy.is_empty() {
            return session.into_iter().take(limit).collect();
        }

        // Inputs are already ORDER BY priority ASC, id ASC per lane.
        let mut s_idx = 0usize;
        let mut l_idx = 0usize;
        let mut selected: Vec<QueuedPrompt> = Vec::with_capacity(limit);

        while selected.len() < limit && (s_idx < session.len() || l_idx < legacy.len()) {
            let next_priority = match (session.get(s_idx), legacy.get(l_idx)) {
                (Some(s), Some(l)) => (s.priority as u8).min(l.priority as u8),
                (Some(s), None) => s.priority as u8,
                (None, Some(l)) => l.priority as u8,
                (None, None) => break,
            };

            // Drain the full same-priority band from each lane head.
            let s_start = s_idx;
            while s_idx < session.len() && session[s_idx].priority as u8 == next_priority {
                s_idx += 1;
            }
            let l_start = l_idx;
            while l_idx < legacy.len() && legacy[l_idx].priority as u8 == next_priority {
                l_idx += 1;
            }

            let remaining = limit - selected.len();
            let band = Self::fair_quota_same_priority(
                &session[s_start..s_idx],
                &legacy[l_start..l_idx],
                remaining,
            );
            selected.extend(band);
        }

        selected
    }

    /// Bounded two-lane fairness among rows that already share one priority.
    /// Session reserve = ceil(limit/2), legacy = floor(limit/2); unused fills
    /// the other lane. Output ordered by id ASC (equal-priority FIFO).
    fn fair_quota_same_priority(
        session: &[QueuedPrompt],
        legacy: &[QueuedPrompt],
        limit: usize,
    ) -> Vec<QueuedPrompt> {
        if limit == 0 {
            return Vec::new();
        }
        if session.is_empty() {
            return legacy.iter().take(limit).cloned().collect();
        }
        if legacy.is_empty() {
            return session.iter().take(limit).cloned().collect();
        }

        let session_quota = (limit + 1) / 2;
        let legacy_quota = limit / 2;

        let mut s_take = session_quota.min(session.len());
        let mut l_take = legacy_quota.min(legacy.len());

        // Give unused reserve to the other lane.
        let used = s_take + l_take;
        if used < limit {
            let spare = limit - used;
            let s_extra = (session.len() - s_take).min(spare);
            s_take += s_extra;
            let spare = limit - s_take - l_take;
            let l_extra = (legacy.len() - l_take).min(spare);
            l_take += l_extra;
        }

        let mut selected: Vec<QueuedPrompt> = Vec::with_capacity(s_take + l_take);
        selected.extend(session.iter().take(s_take).cloned());
        selected.extend(legacy.iter().take(l_take).cloned());
        selected.sort_by_key(|p| p.id);
        selected
    }

    fn query_lane(
        conn: &Connection,
        sql: &str,
        params: &[Box<dyn rusqlite::ToSql>],
    ) -> Result<Vec<QueuedPrompt>> {
        let mut stmt = conn.prepare_cached(sql)?;
        let prompts = stmt
            .query_map(
                rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                Self::prompt_from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(prompts)
    }

    fn require_datetime(s: &str, field: &str, id: i64) -> Result<DateTime<Utc>> {
        Self::parse_datetime(s).ok_or_else(|| {
            crate::error::StoreError::Parse(format!(
                "prompt_queue id={id}: corrupt/unparseable {field} timestamp: {s:?}"
            ))
        })
    }

    fn optional_datetime(s: Option<&str>, field: &str, id: i64) -> Result<Option<DateTime<Utc>>> {
        match s {
            None => Ok(None),
            Some(raw) => Ok(Some(Self::require_datetime(raw, field, id)?)),
        }
    }

    /// Read current highest_stage. Missing/null → Enqueued. Corrupt → Parse error.
    fn read_highest_stage(conn: &Connection, prompt_id: i64) -> Result<DeliveryStage> {
        let current_s: Option<String> = conn
            .query_row(
                "SELECT highest_stage FROM prompt_queue WHERE id = ?",
                params![prompt_id],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten();
        match current_s {
            None => Ok(DeliveryStage::Enqueued),
            Some(ref s) => DeliveryStage::parse(s).ok_or_else(|| {
                crate::error::StoreError::Parse(format!(
                    "prompt_queue id={prompt_id}: corrupt/unknown highest_stage: {s:?}"
                ))
            }),
        }
    }

    /// Resolve next stage under legal-transition rules.
    /// - Higher rank → accept proposed
    /// - Lower rank → keep current (reason-only update; no regression)
    /// - Same rank different siblings → reject (except Partial→Delivered)
    fn resolve_stage_transition(
        current: DeliveryStage,
        proposed: DeliveryStage,
        prompt_id: i64,
    ) -> Result<DeliveryStage> {
        if current == proposed {
            return Ok(current);
        }
        if proposed.rank() > current.rank() {
            return Ok(proposed);
        }
        if current == DeliveryStage::PartiallyDelivered && proposed == DeliveryStage::Delivered {
            return Ok(proposed);
        }
        if proposed.rank() < current.rank() {
            // Keep higher stage; allow pending_reason refresh only.
            return Ok(current);
        }
        Err(crate::error::StoreError::Other(format!(
            "prompt_queue id={prompt_id}: illegal stage transition {current} → {proposed}"
        )))
    }

    /// Cross-process atomic stage stamp (cas-2c5f review 3).
    ///
    /// Uses `BEGIN IMMEDIATE` so read_highest_stage + legal-transition check +
    /// stage/timestamp/reason write are one SQLite write transaction — safe
    /// across independent connections/processes (not just process-local Mutex).
    fn atomic_stage_stamp(
        conn: &Connection,
        prompt_id: i64,
        proposed: DeliveryStage,
        opts: AtomicStampOpts<'_>,
    ) -> Result<()> {
        let tx = crate::shared_db::ImmediateTx::new(conn)?;
        Self::atomic_stage_stamp_in_tx(&tx, prompt_id, proposed, opts)?;
        tx.commit()?;
        Ok(())
    }

    /// Read + resolve + write inside an already-open IMMEDIATE transaction.
    fn atomic_stage_stamp_in_tx(
        tx: &Connection,
        prompt_id: i64,
        proposed: DeliveryStage,
        opts: AtomicStampOpts<'_>,
    ) -> Result<()> {
        let current = Self::read_highest_stage(tx, prompt_id)?;
        let next = Self::resolve_stage_transition(current, proposed, prompt_id)?;
        let now = Utc::now().to_rfc3339();

        // Full Delivered always stamps transport_delivered_at; partial never does.
        let stamp_transport = next == DeliveryStage::Delivered;
        let stamp_processed = opts.set_processed
            || stamp_transport
            || next.is_terminal_non_delivery()
            || next == DeliveryStage::PartiallyDelivered;

        // reason=None clears pending reason fields (Delivered success path).
        // No follow-up UPDATE — keeps the atomic contract intact.
        tx.execute(
            "UPDATE prompt_queue SET
                highest_stage = ?,
                selected_at = COALESCE(selected_at, ?),
                transport_delivered_at = CASE
                    WHEN ? THEN COALESCE(transport_delivered_at, ?)
                    ELSE transport_delivered_at
                END,
                processed_at = CASE
                    WHEN ? THEN COALESCE(processed_at, ?)
                    ELSE processed_at
                END,
                last_pending_reason = ?,
                last_pending_detail = ?,
                broadcast_attempted = COALESCE(?, broadcast_attempted),
                broadcast_succeeded = COALESCE(?, broadcast_succeeded),
                broadcast_failed = COALESCE(?, broadcast_failed)
             WHERE id = ?",
            params![
                next.as_str(),
                now,
                stamp_transport as i64,
                now,
                stamp_processed as i64,
                now,
                opts.reason.map(|r| r.as_str()),
                opts.detail,
                opts.broadcast_attempted.map(|n| n as i64),
                opts.broadcast_succeeded.map(|n| n as i64),
                opts.broadcast_failed.map(|n| n as i64),
                prompt_id,
            ],
        )?;
        Ok(())
    }
}

/// Options for [`SqlitePromptQueueStore::atomic_stage_stamp`].
struct AtomicStampOpts<'a> {
    reason: Option<PendingReason>,
    detail: Option<&'a str>,
    set_processed: bool,
    broadcast_attempted: Option<u32>,
    broadcast_succeeded: Option<u32>,
    broadcast_failed: Option<u32>,
}

impl<'a> AtomicStampOpts<'a> {
    fn reason(reason: PendingReason, detail: Option<&'a str>) -> Self {
        Self {
            reason: Some(reason),
            detail,
            set_processed: false,
            broadcast_attempted: None,
            broadcast_succeeded: None,
            broadcast_failed: None,
        }
    }

    fn clear_reason() -> Self {
        Self {
            reason: None,
            detail: None,
            set_processed: false,
            broadcast_attempted: None,
            broadcast_succeeded: None,
            broadcast_failed: None,
        }
    }
}

impl PromptQueueStore for SqlitePromptQueueStore {
    fn init(&self) -> Result<()> {
        // cas-88d8: concurrent openers race on check-then-ALTER. SQLite
        // auto-commits DDL, so do not wrap ADD COLUMN in ImmediateTx.
        // ensure_column + with_write_retry; indexes only after columns exist.
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            conn.execute_batch(PROMPT_QUEUE_SCHEMA)?;
            let first_lifecycle_migration =
                !crate::shared_db::column_exists(&conn, "prompt_queue", "highest_stage");

            for (col, mig) in [
                ("factory_session", PROMPT_QUEUE_SESSION_MIGRATION),
                ("summary", PROMPT_QUEUE_SUMMARY_MIGRATION),
                ("priority", PROMPT_QUEUE_PRIORITY_MIGRATION),
                ("acked_at", PROMPT_QUEUE_ACKED_AT_MIGRATION),
                ("urgent", PROMPT_QUEUE_URGENT_MIGRATION),
                ("selected_at", PROMPT_QUEUE_SELECTED_AT_MIGRATION),
                ("last_pending_reason", PROMPT_QUEUE_PENDING_REASON_MIGRATION),
                ("last_pending_detail", PROMPT_QUEUE_PENDING_DETAIL_MIGRATION),
                (
                    "transport_delivered_at",
                    PROMPT_QUEUE_TRANSPORT_DELIVERED_AT_MIGRATION,
                ),
                ("highest_stage", PROMPT_QUEUE_HIGHEST_STAGE_MIGRATION),
                (
                    "broadcast_attempted",
                    PROMPT_QUEUE_BROADCAST_ATTEMPTED_MIGRATION,
                ),
                (
                    "broadcast_succeeded",
                    PROMPT_QUEUE_BROADCAST_SUCCEEDED_MIGRATION,
                ),
                ("broadcast_failed", PROMPT_QUEUE_BROADCAST_FAILED_MIGRATION),
                (
                    "delivery_attempts",
                    PROMPT_QUEUE_DELIVERY_ATTEMPTS_MIGRATION,
                ),
                ("next_attempt_at", PROMPT_QUEUE_NEXT_ATTEMPT_AT_MIGRATION),
                ("first_attempt_at", PROMPT_QUEUE_FIRST_ATTEMPT_AT_MIGRATION),
                ("dedupe_key", PROMPT_QUEUE_DEDUPE_KEY_MIGRATION),
            ] {
                crate::shared_db::ensure_column(&conn, "prompt_queue", col, mig)?;
            }

            // Pre-telemetry rows only have the legacy processed_at marker. Hydrate
            // both lifecycle fields so delivery reports stay internally consistent.
            if first_lifecycle_migration {
                conn.execute(
                    "UPDATE prompt_queue
                     SET highest_stage = 'delivered',
                         transport_delivered_at = processed_at
                     WHERE processed_at IS NOT NULL
                       AND highest_stage IS NULL",
                    [],
                )?;
            }

            // Indexes are IF NOT EXISTS — safe under concurrency once columns exist.
            conn.execute_batch(PROMPT_QUEUE_TWO_LANE_INDEXES)?;
            conn.execute_batch(PROMPT_QUEUE_DEDUPE_KEY_INDEX)?;
            conn.execute_batch(PROMPT_QUEUE_MESSAGE_HOT_PATH_INDEXES_MIGRATION)?;
            Ok(())
        })
    }

    fn enqueue(&self, source: &str, target: &str, prompt: &str) -> Result<i64> {
        self.enqueue_full(source, target, prompt, None, None, None)
    }

    fn enqueue_with_session(
        &self,
        source: &str,
        target: &str,
        prompt: &str,
        factory_session: &str,
    ) -> Result<i64> {
        self.enqueue_full(source, target, prompt, Some(factory_session), None, None)
    }

    fn enqueue_urgent_with_outcome(
        &self,
        source: &str,
        target: &str,
        prompt: &str,
        factory_session: Option<&str>,
        summary: Option<&str>,
        priority: Option<NotificationPriority>,
        urgent: bool,
    ) -> Result<EnqueueOutcome> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let duplicate_cutoff =
                (now - chrono::Duration::seconds(PROMPT_DUPLICATE_WINDOW_SECS)).to_rfc3339();
            let prio: i32 = priority.unwrap_or(NotificationPriority::Normal).into();
            let urgent_flag: i64 = if urgent { 1 } else { 0 };

            // cas-6ad2/cas-c061: collapse only the immediate race where a worker
            // repeats an unchanged report while its acknowledgement is in
            // flight. Confirmed or older rows are historical events and must
            // never permanently reserve the body. Supervisors are exempt
            // because repeating an instruction can be intentional; urgent
            // sends are always intentional redelivery.
            if !urgent && source != "supervisor" {
                let delivered_duplicate = tx
                    .query_row(
                        "SELECT id
                         FROM prompt_queue
                         WHERE source = ?
                           AND target = ?
                           AND prompt = ?
                           AND factory_session IS ?
                           AND urgent = 0
                           AND transport_delivered_at IS NOT NULL
                           AND acked_at IS NULL
                           AND highest_stage IS NOT 'confirmed'
                           AND transport_delivered_at >= ?
                         ORDER BY transport_delivered_at DESC, id DESC
                         LIMIT 1",
                        params![
                            source,
                            target,
                            prompt,
                            factory_session,
                            duplicate_cutoff
                        ],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?;
                if let Some(existing_id) = delivered_duplicate {
                    tx.commit()?;
                    return Ok(EnqueueOutcome::SuppressedDuplicate(existing_id));
                }
            }

            tx.execute(
            "INSERT INTO prompt_queue (source, target, prompt, created_at, factory_session, summary, priority, urgent) VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            params![source, target, prompt, now_text, factory_session, summary, prio, urgent_flag],
        )?;

            let id = tx.last_insert_rowid();
            let _ = capture_message_event(&tx, source, target);
            tx.commit()?;
            Ok(EnqueueOutcome::Created(id))
        }) // with_write_retry
    }

    fn enqueue_idempotent(
        &self,
        source: &str,
        target: &str,
        prompt: &str,
        factory_session: Option<&str>,
        summary: Option<&str>,
        priority: Option<NotificationPriority>,
        dedupe_key: &str,
    ) -> Result<EnqueueIdempotentResult> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();
            let prio: i32 = priority.unwrap_or(NotificationPriority::Normal).into();

            let changed = conn.execute(
                "INSERT OR IGNORE INTO prompt_queue
                    (source, target, prompt, created_at, factory_session, summary, priority, urgent, dedupe_key)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?)",
                params![
                    source,
                    target,
                    prompt,
                    now,
                    factory_session,
                    summary,
                    prio,
                    dedupe_key
                ],
            )?;

            if changed > 0 {
                let id = conn.last_insert_rowid();
                let _ = capture_message_event(&conn, source, target);
                return Ok(EnqueueIdempotentResult::Created(id));
            }

            let existing_id: i64 = conn.query_row(
                "SELECT id FROM prompt_queue WHERE dedupe_key = ?",
                params![dedupe_key],
                |row| row.get(0),
            )?;
            Ok(EnqueueIdempotentResult::AlreadyExists(existing_id))
        })
    }

    fn poll_for_target(&self, target: &str, limit: usize) -> Result<Vec<QueuedPrompt>> {
        self.poll_for_target_with_session(target, None, limit)
    }

    fn poll_for_target_with_session(
        &self,
        target: &str,
        factory_session: Option<&str>,
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();

            let (sql, prompt_params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = if let Some(session) =
                factory_session
            {
                (
                    "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session
             FROM prompt_queue
             WHERE processed_at IS NULL
               AND (
                    (factory_session = ? AND (target = ? OR target = 'all_workers'))
                    OR (factory_session IS NULL AND (target = ? OR target = 'all_workers'))
               )
             ORDER BY priority ASC, id ASC
             LIMIT ?",
                    vec![
                        Box::new(session.to_string()),
                        Box::new(target.to_string()),
                        Box::new(target.to_string()),
                        Box::new(limit as i64),
                    ],
                )
            } else {
                (
                    "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session
             FROM prompt_queue
             WHERE (target = ? OR target = 'all_workers') AND processed_at IS NULL
             ORDER BY priority ASC, id ASC
             LIMIT ?",
                    vec![Box::new(target.to_string()), Box::new(limit as i64)],
                )
            };

            let mut stmt = conn.prepare_cached(sql)?;

            let prompts: Vec<QueuedPrompt> = stmt
                .query_map(
                    rusqlite::params_from_iter(prompt_params.iter().map(|p| p.as_ref())),
                    Self::prompt_from_row,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            // Mark them as processed
            if !prompts.is_empty() {
                let ids: Vec<i64> = prompts.iter().map(|p| p.id).collect();
                let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
                let sql = format!(
                    "UPDATE prompt_queue SET processed_at = ?, last_pending_reason = NULL, last_pending_detail = NULL WHERE id IN ({})",
                    placeholders.join(", ")
                );

                let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(now)];
                for id in ids {
                    params.push(Box::new(id));
                }

                conn.execute(
                    &sql,
                    rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                )?;
            }

            Ok(prompts)
        }) // with_write_retry
    }

    fn poll_all(&self, limit: usize) -> Result<Vec<QueuedPrompt>> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();

            let mut stmt = conn.prepare_cached(
            "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session
             FROM prompt_queue
             WHERE processed_at IS NULL
             ORDER BY priority ASC, id ASC
             LIMIT ?",
        )?;

            let prompts: Vec<QueuedPrompt> = stmt
                .query_map(params![limit as i64], Self::prompt_from_row)?
                .collect::<std::result::Result<Vec<_>, _>>()?;

            // Mark them as processed
            if !prompts.is_empty() {
                let ids: Vec<i64> = prompts.iter().map(|p| p.id).collect();
                let placeholders: Vec<String> = ids.iter().map(|_| "?".to_string()).collect();
                let sql = format!(
                    "UPDATE prompt_queue SET processed_at = ?, last_pending_reason = NULL, last_pending_detail = NULL WHERE id IN ({})",
                    placeholders.join(", ")
                );

                let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(now)];
                for id in ids {
                    params.push(Box::new(id));
                }

                conn.execute(
                    &sql,
                    rusqlite::params_from_iter(params.iter().map(|p| p.as_ref())),
                )?;
            }

            Ok(prompts)
        }) // with_write_retry
    }

    fn peek_all(&self, limit: usize) -> Result<Vec<QueuedPrompt>> {
        let conn = self.conn.lock().unwrap();

        let mut stmt = conn.prepare_cached(
            "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session
             FROM prompt_queue
             WHERE processed_at IS NULL
             ORDER BY priority ASC, id ASC
             LIMIT ?",
        )?;

        let prompts = stmt
            .query_map(params![limit as i64], Self::prompt_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(prompts)
    }

    fn peek_for_targets(
        &self,
        targets: &[&str],
        factory_session: Option<&str>,
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>> {
        if targets.is_empty() {
            return Err(StoreError::Other(
                "peek_for_targets requires at least one target; session-wide peeks are not supported"
                    .to_string(),
            ));
        }
        if limit == 0 {
            return Ok(Vec::new());
        }

        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();

        // Legacy path (no session): single-lane target filter.
        let Some(session) = factory_session else {
            let placeholders: Vec<&str> = std::iter::repeat_n("?", targets.len()).collect();
            let sql = format!(
                "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session
                 FROM (
                     SELECT *, ROW_NUMBER() OVER (
                         PARTITION BY target, priority ORDER BY id ASC
                     ) AS cas_target_rn
                     FROM prompt_queue
                     WHERE processed_at IS NULL
                       AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
                       AND target IN ({})
                 )
                 ORDER BY priority ASC, cas_target_rn ASC, id ASC
                 LIMIT ?",
                placeholders.join(", ")
            );
            let mut params: Vec<Box<dyn rusqlite::ToSql>> =
                vec![Box::new(now.clone()) as Box<dyn rusqlite::ToSql>];
            params.extend(
                targets
                    .iter()
                    .map(|t| Box::new(t.to_string()) as Box<dyn rusqlite::ToSql>),
            );
            params.push(Box::new(limit as i64));
            return Self::query_lane(&conn, &sql, &params);
        };

        // Live-session path: two indexable peeks + bounded two-lane merge
        // (cas-2bcb). Each lane is LIMIT-bounded so neither can permanently
        // occupy the caller's window.
        //
        // cas-7210: within each lane, also round-robin across targets
        // instead of a flat `ORDER BY priority ASC, id ASC`. The window
        // function ranks each row by its position within its OWN
        // `(target, priority)` queue (`cas_target_rn`); ordering the final
        // result by `(priority, cas_target_rn, id)` means every target's
        // *oldest* row at a given priority is considered before any
        // target's *second* row at that same priority — `priority` stays
        // the dominant sort key, so the existing "never emit priority P+1
        // while priority ≤P remains" contract is untouched, and for the
        // common case of a single contending target this reduces to
        // exactly the original `(priority, id)` FIFO order (rn increases
        // monotonically with id when there's only one target, so it's a
        // no-op reordering).
        //
        // Without this, a target with a persistent, never-resolving
        // backlog (rows left `processed_at IS NULL` by
        // `record_pending_reason` — AdapterRetryable / GatedNotReady /
        // TargetUnavailable / AwaitingDelivery are all designed to keep
        // retrying, not to resolve on their own) sorts first by id and can
        // fill the ENTIRE `limit` window on every tick, forever. A fresh
        // message to a completely different, actively-working target then
        // never appears in the peeked batch at all — not retried, not
        // logged as failing, simply invisible. Reproduced at
        // `peek_for_targets_gives_active_target_a_slot_despite_another_targets_stuck_backlog`.
        let placeholders: Vec<&str> = std::iter::repeat_n("?", targets.len()).collect();
        let session_sql = format!("SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session
             FROM (
                 SELECT *, ROW_NUMBER() OVER (
                     PARTITION BY target, priority ORDER BY id ASC
                 ) AS cas_target_rn
                 FROM prompt_queue
                 WHERE processed_at IS NULL
                   AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
                   AND factory_session = ?
                   AND target IN ({})
             )
             ORDER BY priority ASC, cas_target_rn ASC, id ASC
             LIMIT ?", placeholders.join(", "));
        let mut session_params: Vec<Box<dyn rusqlite::ToSql>> =
            vec![Box::new(now.clone()), Box::new(session.to_string())];
        session_params.extend(
            targets
                .iter()
                .map(|t| Box::new(t.to_string()) as Box<dyn rusqlite::ToSql>),
        );
        session_params.push(Box::new(limit as i64));
        let session_lane = Self::query_lane(&conn, &session_sql, &session_params)?;

        let legacy_lane = if targets.is_empty() {
            Vec::new()
        } else {
            let placeholders: Vec<&str> = std::iter::repeat_n("?", targets.len()).collect();
            let legacy_sql = format!(
                "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session
                 FROM (
                     SELECT *, ROW_NUMBER() OVER (
                         PARTITION BY target, priority ORDER BY id ASC
                     ) AS cas_target_rn
                     FROM prompt_queue
                     WHERE processed_at IS NULL
                       AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
                       AND factory_session IS NULL
                       AND target IN ({})
                 )
                 ORDER BY priority ASC, cas_target_rn ASC, id ASC
                 LIMIT ?",
                placeholders.join(", ")
            );
            let mut legacy_params: Vec<Box<dyn rusqlite::ToSql>> =
                vec![Box::new(now) as Box<dyn rusqlite::ToSql>];
            legacy_params.extend(
                targets
                    .iter()
                    .map(|t| Box::new(t.to_string()) as Box<dyn rusqlite::ToSql>),
            );
            legacy_params.push(Box::new(limit as i64));
            Self::query_lane(&conn, &legacy_sql, &legacy_params)?
        };

        Ok(Self::merge_two_lane_peeks(session_lane, legacy_lane, limit))
    }

    fn outstanding_for_targets(
        &self,
        targets: &[&str],
        factory_session: Option<&str>,
    ) -> Result<Vec<QueuedPrompt>> {
        if targets.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders = std::iter::repeat_n("?", targets.len())
            .collect::<Vec<_>>()
            .join(", ");
        let session_clause = if factory_session.is_some() {
            "AND (factory_session = ? OR factory_session IS NULL)"
        } else {
            "AND factory_session IS NULL"
        };
        let sql = format!(
            "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session
             FROM prompt_queue
             WHERE processed_at IS NULL
               AND target IN ({placeholders})
               {session_clause}
             ORDER BY priority ASC, id ASC"
        );

        let mut query_params: Vec<Box<dyn rusqlite::ToSql>> = targets
            .iter()
            .map(|target| Box::new((*target).to_string()) as Box<dyn rusqlite::ToSql>)
            .collect();
        if let Some(session) = factory_session {
            query_params.push(Box::new(session.to_string()));
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql)?;
        let prompts = stmt
            .query_map(
                rusqlite::params_from_iter(query_params.iter().map(|p| p.as_ref())),
                Self::prompt_from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(prompts)
    }

    fn latest_created_at_for_targets_from_sources(
        &self,
        sources: &[&str],
        targets: &[&str],
        factory_session: Option<&str>,
    ) -> Result<HashMap<String, DateTime<Utc>>> {
        if sources.is_empty() || targets.is_empty() {
            return Ok(HashMap::new());
        }

        let source_placeholders = std::iter::repeat_n("?", sources.len())
            .collect::<Vec<_>>()
            .join(", ");
        let target_placeholders = std::iter::repeat_n("?", targets.len())
            .collect::<Vec<_>>()
            .join(", ");
        let session_clause = if factory_session.is_some() {
            "AND (factory_session = ? OR factory_session IS NULL)"
        } else {
            "AND factory_session IS NULL"
        };
        let sql = format!(
            "SELECT target, created_at
             FROM prompt_queue
             WHERE id IN (
                 SELECT MAX(id)
                 FROM prompt_queue
                 WHERE source IN ({source_placeholders})
                   AND target IN ({target_placeholders})
                   {session_clause}
                 GROUP BY target
             )"
        );

        let mut query_params: Vec<Box<dyn rusqlite::ToSql>> = sources
            .iter()
            .map(|value| Box::new((*value).to_string()) as Box<dyn rusqlite::ToSql>)
            .chain(
                targets
                    .iter()
                    .map(|value| Box::new((*value).to_string()) as Box<dyn rusqlite::ToSql>),
            )
            .collect();
        if let Some(session) = factory_session {
            query_params.push(Box::new(session.to_string()));
        }

        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(
            rusqlite::params_from_iter(query_params.iter().map(|p| p.as_ref())),
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?;
        let mut latest = HashMap::new();
        for row in rows {
            let (target, created_at) = row?;
            if latest.contains_key(&target) {
                continue;
            }
            if let Some(created_at) = Self::parse_datetime(&created_at) {
                latest.insert(target, created_at);
            }
        }
        Ok(latest)
    }

    fn mark_processed(&self, prompt_id: i64) -> Result<()> {
        // Legacy API: sets processed_at only. Does **not** stamp authoritative
        // transport delivery (cas-2c5f). Prefer mark_transport_delivered /
        // mark_dropped / mark_suppressed / mark_abandoned on the delivery path.
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE prompt_queue SET processed_at = ? WHERE id = ?",
                params![now, prompt_id],
            )?;
            Ok(())
        }) // with_write_retry
    }

    fn ack(&self, prompt_id: i64) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();
            // Atomic stage advance to Confirmed (legal from Delivered or Partial).
            let _ = Self::atomic_stage_stamp(
                &conn,
                prompt_id,
                DeliveryStage::Confirmed,
                AtomicStampOpts::clear_reason(),
            );
            conn.execute(
                "UPDATE prompt_queue SET acked_at = ? WHERE id = ? AND acked_at IS NULL",
                params![now, prompt_id],
            )?;
            // rows_affected == 0 means either not found or already acked — both idempotent
            Ok(())
        }) // with_write_retry
    }

    fn ack_delivered_for_recipient(
        &self,
        recipient_aliases: &[&str],
        sender_aliases: &[&str],
        factory_session: Option<&str>,
    ) -> Result<usize> {
        if recipient_aliases.is_empty() || sender_aliases.is_empty() {
            return Ok(0);
        }

        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let now = Utc::now().to_rfc3339();
            let recipient_placeholders =
                std::iter::repeat_n("?", recipient_aliases.len()).collect::<Vec<_>>();
            let sender_placeholders =
                std::iter::repeat_n("?", sender_aliases.len()).collect::<Vec<_>>();
            let session_clause = if factory_session.is_some() {
                "AND (factory_session = ? OR factory_session IS NULL)"
            } else {
                "AND factory_session IS NULL"
            };
            let sql = format!(
                "UPDATE prompt_queue
                 SET acked_at = ?,
                     highest_stage = 'confirmed',
                     last_pending_reason = NULL,
                     last_pending_detail = NULL
                 WHERE acked_at IS NULL
                   AND transport_delivered_at IS NOT NULL
                   AND target IN ({})
                   AND source IN ({})
                   {session_clause}",
                recipient_placeholders.join(", "),
                sender_placeholders.join(", "),
            );

            let mut query_params: Vec<Box<dyn rusqlite::ToSql>> =
                Vec::with_capacity(1 + recipient_aliases.len() + sender_aliases.len() + 1);
            query_params.push(Box::new(now));
            query_params.extend(
                recipient_aliases
                    .iter()
                    .map(|value| Box::new((*value).to_string()) as Box<dyn rusqlite::ToSql>),
            );
            query_params.extend(
                sender_aliases
                    .iter()
                    .map(|value| Box::new((*value).to_string()) as Box<dyn rusqlite::ToSql>),
            );
            if let Some(session) = factory_session {
                query_params.push(Box::new(session.to_string()));
            }

            Ok(conn.execute(
                &sql,
                rusqlite::params_from_iter(query_params.iter().map(|value| value.as_ref())),
            )?)
        })
    }

    fn unacked(&self, timeout_secs: i64, limit: usize) -> Result<Vec<QueuedPrompt>> {
        let conn = self.conn.lock().unwrap();
        let cutoff = (Utc::now() - chrono::Duration::seconds(timeout_secs)).to_rfc3339();

        let mut stmt = conn.prepare_cached(
            "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session
             FROM prompt_queue
             WHERE processed_at IS NOT NULL
               AND processed_at < ?
               AND acked_at IS NULL
             ORDER BY priority ASC, id ASC
             LIMIT ?",
        )?;

        let prompts = stmt
            .query_map(params![cutoff, limit as i64], Self::prompt_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(prompts)
    }

    fn message_status(&self, prompt_id: i64) -> Result<Option<MessageStatus>> {
        // Legacy ladder: processed_at / acked_at only (includes non-delivery
        // drains). Structured stage uses separate columns.
        let conn = self.conn.lock().unwrap();
        let result = conn.query_row(
            "SELECT processed_at, acked_at FROM prompt_queue WHERE id = ?",
            params![prompt_id],
            |row| {
                let processed_at: Option<String> = row.get(0)?;
                let acked_at: Option<String> = row.get(1)?;
                Ok((processed_at, acked_at))
            },
        );
        match result {
            Ok((_, Some(_))) => Ok(Some(MessageStatus::Confirmed)),
            Ok((Some(_), None)) => Ok(Some(MessageStatus::Delivered)),
            Ok((None, _)) => Ok(Some(MessageStatus::Pending)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    fn message_delivery_report(&self, prompt_id: i64) -> Result<Option<MessageDeliveryReport>> {
        let conn = self.conn.lock().unwrap();

        let row = conn.query_row(
            "SELECT id, source, target, created_at, processed_at, factory_session,
                    priority, acked_at, urgent, selected_at, last_pending_reason,
                    last_pending_detail, transport_delivered_at, highest_stage,
                    broadcast_attempted, broadcast_succeeded, broadcast_failed
             FROM prompt_queue WHERE id = ?",
            params![prompt_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, u8>(6).unwrap_or(2),
                    row.get::<_, Option<String>>(7)?,
                    row.get::<_, i64>(8).map(|v| v != 0).unwrap_or(false),
                    row.get::<_, Option<String>>(9).unwrap_or(None),
                    row.get::<_, Option<String>>(10).unwrap_or(None),
                    row.get::<_, Option<String>>(11).unwrap_or(None),
                    row.get::<_, Option<String>>(12).unwrap_or(None),
                    row.get::<_, Option<String>>(13).unwrap_or(None),
                    row.get::<_, Option<i64>>(14).unwrap_or(None),
                    row.get::<_, Option<i64>>(15).unwrap_or(None),
                    row.get::<_, Option<i64>>(16).unwrap_or(None),
                ))
            },
        );

        let (
            id,
            source,
            target,
            created_at_s,
            processed_at_s,
            factory_session,
            priority,
            acked_at_s,
            urgent,
            selected_at_s,
            stored_reason,
            stored_detail,
            transport_delivered_s,
            highest_stage_s,
            bc_attempted,
            bc_succeeded,
            bc_failed,
        ) = match row {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        let enqueued_at = Self::require_datetime(&created_at_s, "created_at", id)?;
        let selected_at = Self::optional_datetime(selected_at_s.as_deref(), "selected_at", id)?;
        let delivered_at = Self::optional_datetime(
            transport_delivered_s.as_deref(),
            "transport_delivered_at",
            id,
        )?;
        let confirmed_at = Self::optional_datetime(acked_at_s.as_deref(), "acked_at", id)?;
        let legacy_processed = processed_at_s.is_some();

        let legacy_status = if confirmed_at.is_some() {
            MessageStatus::Confirmed
        } else if legacy_processed {
            MessageStatus::Delivered
        } else {
            MessageStatus::Pending
        };

        let mut stage = match highest_stage_s {
            None => DeliveryStage::Enqueued,
            Some(ref s) => DeliveryStage::parse(s).ok_or_else(|| {
                crate::error::StoreError::Parse(format!(
                    "prompt_queue id={id}: corrupt/unknown highest_stage: {s:?}"
                ))
            })?,
        };

        if stage == DeliveryStage::Delivered && delivered_at.is_none() {
            return Err(crate::error::StoreError::Parse(format!(
                "prompt_queue id={id}: invariant violated: stage=delivered without transport_delivered_at"
            )));
        }
        if stage.is_terminal_non_delivery() && delivered_at.is_some() {
            return Err(crate::error::StoreError::Parse(format!(
                "prompt_queue id={id}: invariant violated: stage={stage} with transport_delivered_at"
            )));
        }
        if stage == DeliveryStage::PartiallyDelivered && delivered_at.is_some() {
            return Err(crate::error::StoreError::Parse(format!(
                "prompt_queue id={id}: invariant violated: partially_delivered with transport_delivered_at"
            )));
        }

        let stored_pending = stored_reason.as_deref().and_then(PendingReason::parse);
        let (pending_reason, pending_detail) = match stage {
            DeliveryStage::Confirmed => (None, None),
            DeliveryStage::Delivered => (
                Some(PendingReason::AwaitingAck),
                Some("transport delivered; waiting for message_ack".into()),
            ),
            DeliveryStage::PartiallyDelivered => (
                Some(stored_pending.unwrap_or(PendingReason::PartialBroadcast)),
                stored_detail,
            ),
            DeliveryStage::Dropped => (
                Some(stored_pending.unwrap_or(PendingReason::DroppedDeadSource)),
                stored_detail,
            ),
            DeliveryStage::Suppressed => (
                Some(stored_pending.unwrap_or(PendingReason::SuppressedIdle)),
                stored_detail,
            ),
            DeliveryStage::Abandoned => (
                Some(stored_pending.unwrap_or(PendingReason::AbandonedUnknownTarget)),
                stored_detail,
            ),
            DeliveryStage::Gated | DeliveryStage::Selected | DeliveryStage::Enqueued => {
                if let Some(reason) = stored_pending {
                    (Some(reason), stored_detail)
                } else {
                    (
                        Some(PendingReason::AwaitingDelivery),
                        Some("awaiting authoritative delivery-path observation".into()),
                    )
                }
            }
        };

        if confirmed_at.is_some() {
            stage = DeliveryStage::Confirmed;
        }

        Ok(Some(MessageDeliveryReport {
            id,
            legacy_status,
            stage,
            source,
            target,
            factory_session,
            priority,
            urgent,
            enqueued_at,
            selected_at,
            delivered_at,
            confirmed_at,
            pending_reason,
            pending_detail,
            broadcast_attempted: bc_attempted.map(|n| n as u32),
            broadcast_succeeded: bc_succeeded.map(|n| n as u32),
            broadcast_failed: bc_failed.map(|n| n as u32),
            wake: ObservationStatus::Unobserved,
            reaction: ObservationStatus::Unobserved,
        }))
    }

    fn record_selected(&self, prompt_id: i64) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            Self::atomic_stage_stamp(
                &conn,
                prompt_id,
                DeliveryStage::Selected,
                AtomicStampOpts::clear_reason(),
            )
        })
    }

    fn record_pending_reason(
        &self,
        prompt_id: i64,
        reason: PendingReason,
        detail: Option<&str>,
    ) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            Self::atomic_stage_stamp(
                &conn,
                prompt_id,
                reason.implied_stage(),
                AtomicStampOpts::reason(reason, detail),
            )
        })
    }

    fn record_retry(
        &self,
        prompt_id: i64,
        reason: PendingReason,
        detail: Option<&str>,
    ) -> Result<PromptRetryDisposition> {
        SqlitePromptQueueStore::record_retry(self, prompt_id, reason, detail)
    }

    fn mark_transport_delivered(&self, prompt_id: i64) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            // reason=None clears last_pending_* inside the same ImmediateTx UPDATE.
            Self::atomic_stage_stamp(
                &conn,
                prompt_id,
                DeliveryStage::Delivered,
                AtomicStampOpts {
                    reason: None,
                    detail: None,
                    set_processed: true,
                    broadcast_attempted: None,
                    broadcast_succeeded: None,
                    broadcast_failed: None,
                },
            )
        })
    }

    fn mark_broadcast_outcome(
        &self,
        prompt_id: i64,
        attempted: u32,
        succeeded: u32,
        failed: u32,
        detail: Option<&str>,
    ) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            if attempted == 0 {
                return Self::atomic_stage_stamp(
                    &conn,
                    prompt_id,
                    DeliveryStage::Selected,
                    AtomicStampOpts {
                        reason: Some(PendingReason::NoIntendedRecipients),
                        detail: detail.or(Some("all_workers broadcast: zero intended recipients")),
                        set_processed: false,
                        broadcast_attempted: Some(0),
                        broadcast_succeeded: Some(0),
                        broadcast_failed: Some(0),
                    },
                );
            }
            if succeeded == attempted {
                // Full success: reason=None clears pending fields atomically.
                return Self::atomic_stage_stamp(
                    &conn,
                    prompt_id,
                    DeliveryStage::Delivered,
                    AtomicStampOpts {
                        reason: None,
                        detail: None,
                        set_processed: true,
                        broadcast_attempted: Some(attempted),
                        broadcast_succeeded: Some(succeeded),
                        broadcast_failed: Some(failed),
                    },
                );
            }
            if succeeded == 0 {
                return Self::atomic_stage_stamp(
                    &conn,
                    prompt_id,
                    DeliveryStage::Selected,
                    AtomicStampOpts {
                        reason: Some(PendingReason::AdapterRetryable),
                        detail: detail.or(Some("all_workers broadcast: zero successes")),
                        set_processed: false,
                        broadcast_attempted: Some(attempted),
                        broadcast_succeeded: Some(0),
                        broadcast_failed: Some(failed),
                    },
                );
            }
            let summary = detail.unwrap_or("all_workers partial delivery");
            let detail_owned =
                format!("{summary}: attempted={attempted} succeeded={succeeded} failed={failed}");
            Self::atomic_stage_stamp(
                &conn,
                prompt_id,
                DeliveryStage::PartiallyDelivered,
                AtomicStampOpts {
                    reason: Some(PendingReason::PartialBroadcast),
                    detail: Some(detail_owned.as_str()),
                    set_processed: true,
                    broadcast_attempted: Some(attempted),
                    broadcast_succeeded: Some(succeeded),
                    broadcast_failed: Some(failed),
                },
            )
        })
    }

    fn mark_dropped(&self, prompt_id: i64, detail: Option<&str>) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            Self::atomic_stage_stamp(
                &conn,
                prompt_id,
                DeliveryStage::Dropped,
                AtomicStampOpts {
                    reason: Some(PendingReason::DroppedDeadSource),
                    detail,
                    set_processed: true,
                    broadcast_attempted: None,
                    broadcast_succeeded: None,
                    broadcast_failed: None,
                },
            )
        })
    }

    fn mark_suppressed(&self, prompt_id: i64, detail: Option<&str>) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            Self::atomic_stage_stamp(
                &conn,
                prompt_id,
                DeliveryStage::Suppressed,
                AtomicStampOpts {
                    reason: Some(PendingReason::SuppressedIdle),
                    detail,
                    set_processed: true,
                    broadcast_attempted: None,
                    broadcast_succeeded: None,
                    broadcast_failed: None,
                },
            )
        })
    }

    fn mark_abandoned(&self, prompt_id: i64, detail: Option<&str>) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            Self::atomic_stage_stamp(
                &conn,
                prompt_id,
                DeliveryStage::Abandoned,
                AtomicStampOpts {
                    reason: Some(PendingReason::AbandonedUnknownTarget),
                    detail,
                    set_processed: true,
                    broadcast_attempted: None,
                    broadcast_succeeded: None,
                    broadcast_failed: None,
                },
            )
        })
    }

    fn pending_count(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM prompt_queue WHERE processed_at IS NULL",
            [],
            |row| row.get(0),
        )?;

        Ok(count as usize)
    }

    fn abandon_pending_older_than(&self, older_than_secs: i64) -> Result<usize> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let cutoff = (Utc::now() - chrono::Duration::seconds(older_than_secs)).to_rfc3339();
            let now = Utc::now().to_rfc3339();
            let detail = format!(
                "expired by explicit queue remediation (older_than_secs={older_than_secs})"
            );
            let rows = conn.execute(
                "UPDATE prompt_queue
                 SET processed_at = COALESCE(processed_at, ?),
                     highest_stage = 'abandoned',
                     last_pending_reason = 'abandoned_unknown_target',
                     last_pending_detail = ?,
                     next_attempt_at = NULL
                 WHERE processed_at IS NULL AND created_at < ?",
                params![now, detail, cutoff],
            )?;
            Ok(rows)
        })
    }

    fn abandon_ineligible_session_targets(
        &self,
        targets: &[&str],
        factory_session: &str,
        older_than_secs: i64,
    ) -> Result<usize> {
        if targets.is_empty() {
            return Ok(0);
        }
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let cutoff = (Utc::now() - chrono::Duration::seconds(older_than_secs)).to_rfc3339();
            let now = Utc::now().to_rfc3339();
            let placeholders = std::iter::repeat_n("?", targets.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "UPDATE prompt_queue
                 SET processed_at = COALESCE(processed_at, ?),
                     highest_stage = 'abandoned',
                     last_pending_reason = 'abandoned_unknown_target',
                     last_pending_detail = 'target no longer belongs to factory session',
                     next_attempt_at = NULL
                 WHERE processed_at IS NULL
                   AND factory_session = ?
                   AND created_at < ?
                   AND target NOT IN ({placeholders})"
            );
            let mut query_params: Vec<Box<dyn rusqlite::ToSql>> = vec![
                Box::new(now),
                Box::new(factory_session.to_string()),
                Box::new(cutoff),
            ];
            query_params.extend(
                targets
                    .iter()
                    .map(|target| Box::new((*target).to_string()) as Box<dyn rusqlite::ToSql>),
            );
            let rows = conn.execute(
                &sql,
                rusqlite::params_from_iter(query_params.iter().map(|param| param.as_ref())),
            )?;
            Ok(rows)
        })
    }

    fn clear(&self) -> Result<usize> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let rows = conn.execute("DELETE FROM prompt_queue", [])?;
            Ok(rows)
        }) // with_write_retry
    }

    fn cleanup_old(&self, older_than_secs: i64) -> Result<usize> {
        crate::shared_db::with_write_retry(|| {
            let conn = self.conn.lock().unwrap();
            let cutoff = (Utc::now() - chrono::Duration::seconds(older_than_secs)).to_rfc3339();

            let rows = conn.execute(
                "DELETE FROM prompt_queue WHERE processed_at IS NOT NULL AND processed_at < ?",
                params![cutoff],
            )?;

            Ok(rows)
        }) // with_write_retry
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::prompt_queue_store::*;
    use rusqlite::params;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, SqlitePromptQueueStore) {
        let temp = TempDir::new().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        (temp, store)
    }

    #[test]
    fn test_enqueue_and_poll() {
        let (_temp, store) = create_test_store();

        // Queue a prompt
        let id = store
            .enqueue("supervisor", "swift-fox", "Hello worker!")
            .unwrap();
        assert!(id > 0);

        // Poll should return it
        let prompts = store.poll_all(10).unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].target, "swift-fox");
        assert_eq!(prompts[0].prompt, "Hello worker!");

        // Polling again should return empty (already processed)
        let prompts = store.poll_all(10).unwrap();
        assert!(prompts.is_empty());
    }

    #[test]
    fn test_all_workers_target() {
        let (_temp, store) = create_test_store();

        // Queue to all_workers
        store
            .enqueue("supervisor", "all_workers", "Everyone listen up!")
            .unwrap();

        // Any worker should see it
        let prompts = store.poll_for_target("swift-fox", 10).unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].target, "all_workers");
    }

    #[test]
    fn test_peek_does_not_process() {
        let (_temp, store) = create_test_store();

        store.enqueue("supervisor", "worker-1", "Test").unwrap();

        // Peek should return prompt
        let prompts = store.peek_all(10).unwrap();
        assert_eq!(prompts.len(), 1);

        // Peek again should still return it
        let prompts = store.peek_all(10).unwrap();
        assert_eq!(prompts.len(), 1);

        // Pending count should be 1
        assert_eq!(store.pending_count().unwrap(), 1);
    }

    #[test]
    fn latest_supervisor_message_includes_processed_rows_and_is_session_scoped() {
        let (_temp, store) = create_test_store();
        let expected = store
            .enqueue_with_session("supervisor", "worker-1", "stand by", "session-a")
            .unwrap();
        store.mark_processed(expected).unwrap();
        let foreign_session = store
            .enqueue_with_session("supervisor", "worker-1", "wrong session", "session-b")
            .unwrap();
        let wrong_source = store
            .enqueue_with_session("another-worker", "worker-1", "wrong source", "session-a")
            .unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE prompt_queue SET created_at = ? WHERE id = ?",
                params!["2026-07-22T12:00:01Z", expected],
            )
            .unwrap();
            conn.execute(
                "UPDATE prompt_queue SET created_at = ? WHERE id = ?",
                params!["2026-07-22T12:00:02Z", foreign_session],
            )
            .unwrap();
            conn.execute(
                "UPDATE prompt_queue SET created_at = ? WHERE id = ?",
                params!["2026-07-22T12:00:03Z", wrong_source],
            )
            .unwrap();
        }

        let latest = store
            .latest_created_at_for_targets_from_sources(
                &["supervisor"],
                &["worker-1"],
                Some("session-a"),
            )
            .unwrap();

        assert_eq!(latest.len(), 1);
        assert_eq!(
            latest["worker-1"],
            chrono::DateTime::parse_from_rfc3339("2026-07-22T12:00:01Z")
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn test_fifo_ordering() {
        let (_temp, store) = create_test_store();

        store.enqueue("supervisor", "worker", "First").unwrap();
        store.enqueue("supervisor", "worker", "Second").unwrap();
        store.enqueue("supervisor", "worker", "Third").unwrap();

        let prompts = store.poll_all(10).unwrap();
        assert_eq!(prompts.len(), 3);
        assert_eq!(prompts[0].prompt, "First");
        assert_eq!(prompts[1].prompt, "Second");
        assert_eq!(prompts[2].prompt, "Third");
    }

    #[test]
    fn test_retry_semantics_when_not_marked_processed() {
        let (_temp, store) = create_test_store();

        let prompt_id = store.enqueue("supervisor", "worker-1", "Retry me").unwrap();

        // Simulate failed injection: prompt is read via peek but not acked.
        let pending = store.peek_all(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(store.pending_count().unwrap(), 1);

        // Prompt remains available for retry.
        let retry_pending = store.peek_all(10).unwrap();
        assert_eq!(retry_pending.len(), 1);
        assert_eq!(retry_pending[0].id, prompt_id);

        // Simulate successful retry path by explicitly acknowledging it.
        store.mark_processed(prompt_id).unwrap();
        assert_eq!(store.pending_count().unwrap(), 0);
    }

    /// cas-6ad2: an exact worker report that already reached the supervisor
    /// must not become a fresh injectable turn merely because the worker
    /// re-enqueued the same stale report while waiting for an acknowledgement.
    #[test]
    fn delivered_worker_report_is_not_selected_again() {
        let (_temp, store) = create_test_store();
        let first = store
            .enqueue_with_session(
                "worker-1",
                "supervisor",
                "cas-b769 complete; MERGE NEEDED",
                "factory-session",
            )
            .unwrap();
        store.mark_transport_delivered(first).unwrap();

        let duplicate = store
            .enqueue_urgent_with_outcome(
                "worker-1",
                "supervisor",
                "cas-b769 complete; MERGE NEEDED",
                Some("factory-session"),
                None,
                None,
                false,
            )
            .unwrap();
        assert_eq!(
            duplicate,
            EnqueueOutcome::SuppressedDuplicate(first),
            "an exact recent delivered report should expose suppression and reuse the row id"
        );

        let selected = store
            .peek_for_targets(&["supervisor"], Some("factory-session"), 10)
            .unwrap();
        assert!(
            selected.is_empty(),
            "an already-delivered exact report must be terminal, not selected again: {selected:?}"
        );
    }

    #[test]
    fn recipient_response_confirms_only_delivered_counterparty_messages() {
        let (_temp, store) = create_test_store();
        let consumed = store
            .enqueue_with_session(
                "supervisor",
                "worker-1",
                "start cas-6ad2",
                "factory-session",
            )
            .unwrap();
        let still_pending = store
            .enqueue_with_session(
                "supervisor",
                "worker-1",
                "later instruction",
                "factory-session",
            )
            .unwrap();
        let other_worker = store
            .enqueue_with_session(
                "supervisor",
                "worker-2",
                "unrelated instruction",
                "factory-session",
            )
            .unwrap();
        store.mark_transport_delivered(consumed).unwrap();

        let confirmed = store
            .ack_delivered_for_recipient(
                &["worker-1"],
                &["supervisor", "display-supervisor"],
                Some("factory-session"),
            )
            .unwrap();
        assert_eq!(confirmed, 1);
        assert_eq!(
            store
                .message_delivery_report(consumed)
                .unwrap()
                .unwrap()
                .stage,
            DeliveryStage::Confirmed
        );
        assert_eq!(
            store
                .message_delivery_report(still_pending)
                .unwrap()
                .unwrap()
                .stage,
            DeliveryStage::Enqueued,
            "a response must not confirm a message that never reached transport delivery"
        );
        assert_eq!(
            store
                .message_delivery_report(other_worker)
                .unwrap()
                .unwrap()
                .stage,
            DeliveryStage::Enqueued,
            "recipient aliases must prevent confirming another worker's mail"
        );
    }

    #[test]
    fn test_session_isolation_peek_for_targets() {
        let (_temp, store) = create_test_store();

        // Session A messages
        store
            .enqueue_with_session("supervisor-a", "worker-a1", "Task for A1", "session-a")
            .unwrap();
        store
            .enqueue_with_session("supervisor-a", "worker-a2", "Task for A2", "session-a")
            .unwrap();

        // Session B messages
        store
            .enqueue_with_session("supervisor-b", "worker-b1", "Task for B1", "session-b")
            .unwrap();

        // Legacy message (no session tag)
        store
            .enqueue("supervisor-a", "worker-a1", "Legacy msg")
            .unwrap();

        // Session A should only see its own messages + legacy for its targets
        let targets_a = &["supervisor-a", "worker-a1", "worker-a2", "all_workers"];
        let prompts_a = store
            .peek_for_targets(targets_a, Some("session-a"), 10)
            .unwrap();
        assert_eq!(prompts_a.len(), 3); // 2 session-tagged + 1 legacy by target match
        assert!(prompts_a.iter().all(|p| p.target != "worker-b1"));
        assert_eq!(
            prompts_a
                .iter()
                .filter(|p| p.factory_session.as_deref() == Some("session-a"))
                .count(),
            2
        );

        // Session B should only see its own messages
        let targets_b = &["supervisor-b", "worker-b1", "all_workers"];
        let prompts_b = store
            .peek_for_targets(targets_b, Some("session-b"), 10)
            .unwrap();
        assert_eq!(prompts_b.len(), 1);
        assert_eq!(prompts_b[0].target, "worker-b1");
        assert_eq!(prompts_b[0].factory_session.as_deref(), Some("session-b"));
    }

    #[test]
    fn peek_for_targets_rejects_empty_target_universe() {
        let (_temp, store) = create_test_store();
        store
            .enqueue_with_session("supervisor", "worker", "session row", "session-a")
            .unwrap();

        let error = store
            .peek_for_targets(&[], Some("session-a"), 10)
            .expect_err("session-only peeks must fail loudly");
        assert!(
            error
                .to_string()
                .contains("requires at least one target; session-wide peeks are not supported"),
            "unexpected error: {error}"
        );
    }

    /// Regression test for cas-7210 ("active workers stop receiving ALL
    /// messages mid-session; all_workers broadcast 0-for-4").
    ///
    /// BEFORE THE FIX, this exact test failed: `peek_for_targets`'s session
    /// lane had NO per-target fairness — a single flat
    /// `ORDER BY priority ASC, id ASC LIMIT ?` across the WHOLE session,
    /// regardless of which target each row was for. Any row left with
    /// `processed_at IS NULL` by `record_pending_reason` (AdapterRetryable /
    /// GatedNotReady / TargetUnavailable / AwaitingDelivery are all designed
    /// to keep retrying, not to resolve on their own) stayed in that pool
    /// indefinitely. Once `limit` (10 in production —
    /// queue_and_events.rs:308) such stuck rows accumulated for ANY
    /// target(s), they occupied the ENTIRE window every tick (oldest id
    /// always sorts first), and a genuinely fresh message for a completely
    /// different, actively-working target never appeared in the peeked
    /// batch at all: not delivered, not retried-and-eventually-seen, simply
    /// absent. `process_prompt_queue` only sees what `peek_for_targets`
    /// returns, so the message was never even attempted — matching the
    /// reported signature of "reports success/registration while silently
    /// doing nothing." Confirmed by running this exact test against the
    /// pre-fix query: it failed with `fresh_id` absent from a 10-row result
    /// entirely made of `stuck-target` rows.
    ///
    /// AFTER THE FIX, the session/legacy lane queries rank each row by its
    /// position within its own `(target, priority)` queue
    /// (`ROW_NUMBER() OVER (PARTITION BY target, priority ORDER BY id)`)
    /// and order the final candidates by `(priority, rank, id)` instead of
    /// flat `(priority, id)`. Priority remains the dominant sort key
    /// (nothing about existing priority-ordering guarantees changes); within
    /// one priority band, every target's *oldest* pending row is now
    /// considered before any target's *second* row, so one target's
    /// backlog — however large — can delay but never fully exclude another
    /// target's traffic. For the ordinary single-target case this is a
    /// no-op reordering (rank increases monotonically with id when only one
    /// target is present), which is why the unrelated multi-row-per-target
    /// tests elsewhere in this module are unaffected.
    ///
    /// This one mechanism explains both reported symptoms: an active worker
    /// can stop receiving ALL new messages mid-session (its fresh direct
    /// messages never surface past another target's stuck backlog), and an
    /// `all_workers` broadcast can behave the same way (its row is itself
    /// just one more entry competing for the same starved window).
    #[test]
    fn peek_for_targets_gives_active_target_a_slot_despite_another_targets_stuck_backlog() {
        let (_temp, store) = create_test_store();
        let limit = 10usize;

        // Fill the session with `limit` rows for a target that will never
        // resolve — exactly what a persistent structural delivery failure
        // (not a one-off transient blip) leaves behind via
        // `record_pending_reason`, which deliberately does not set
        // `processed_at` so the row is retried.
        for i in 0..limit {
            let id = store
                .enqueue_with_session(
                    "supervisor",
                    "stuck-target",
                    &format!("stuck message {i}"),
                    "session-a",
                )
                .unwrap();
            store
                .record_pending_reason(
                    id,
                    PendingReason::AdapterRetryable,
                    Some("simulated persistent delivery failure"),
                )
                .unwrap();
        }

        // A brand-new message to a COMPLETELY DIFFERENT, actively-working
        // target, enqueued after all the stuck rows.
        let fresh_id = store
            .enqueue_with_session(
                "supervisor",
                "active-worker",
                "fresh message for the active worker",
                "session-a",
            )
            .unwrap();

        // This is exactly the call process_prompt_queue makes: all live
        // targets for the session, limit=10 (queue_and_events.rs:308).
        let targets = &["supervisor", "all_workers", "director", "active-worker", "stuck-target"];
        let peeked = store
            .peek_for_targets(targets, Some("session-a"), limit)
            .unwrap();

        assert!(
            peeked.iter().any(|p| p.id == fresh_id),
            "cas-7210 regression: a fresh message to an active, unrelated target \
             must appear in the peeked batch even when another target has a large \
             never-resolving backlog. Got {} rows, none matching fresh_id={fresh_id}: {peeked:?}",
            peeked.len()
        );

        // A second fresh message to the same active target must also get
        // through on the very next peek — the fix isn't a one-time fluke.
        let second_fresh_id = store
            .enqueue_with_session(
                "supervisor",
                "active-worker",
                "second fresh message, must not be starved",
                "session-a",
            )
            .unwrap();
        let peeked_again = store
            .peek_for_targets(targets, Some("session-a"), limit)
            .unwrap();
        assert!(
            peeked_again.iter().any(|p| p.id == second_fresh_id),
            "the fix must hold for every subsequent fresh message, not just the first"
        );
    }

    /// AC3-focused variant of the cas-7210 regression above: the fresh
    /// message that must not be starved is itself an `all_workers`
    /// broadcast row, directly covering the reported "all_workers broadcast
    /// 0-for-4" symptom (a broadcast row is, from `peek_for_targets`'
    /// perspective, just one more row competing for the same window — the
    /// starvation mechanism and fix are identical to a direct message).
    #[test]
    fn peek_for_targets_gives_all_workers_broadcast_a_slot_despite_stuck_backlog() {
        let (_temp, store) = create_test_store();
        let limit = 10usize;

        for i in 0..limit {
            let id = store
                .enqueue_with_session(
                    "supervisor",
                    "stuck-target",
                    &format!("stuck message {i}"),
                    "session-a",
                )
                .unwrap();
            store
                .record_pending_reason(
                    id,
                    PendingReason::AdapterRetryable,
                    Some("simulated persistent delivery failure"),
                )
                .unwrap();
        }

        let broadcast_id = store
            .enqueue_with_session(
                "supervisor",
                "all_workers",
                "checkpoint broadcast",
                "session-a",
            )
            .unwrap();

        let targets = &["supervisor", "all_workers", "director", "stuck-target"];
        let peeked = store
            .peek_for_targets(targets, Some("session-a"), limit)
            .unwrap();

        assert!(
            peeked.iter().any(|p| p.id == broadcast_id),
            "cas-7210 AC3 regression: an all_workers broadcast must appear in the \
             peeked batch even when another target has a large never-resolving \
             backlog. Got {} rows, none matching broadcast_id={broadcast_id}: {peeked:?}",
            peeked.len()
        );
    }

    #[test]
    fn test_enqueue_with_session_tags_correctly() {
        let (_temp, store) = create_test_store();

        store
            .enqueue_with_session("sup", "worker-1", "Hello", "my-session")
            .unwrap();

        // peek_all still sees it (no session filter)
        let all = store.peek_all(10).unwrap();
        assert_eq!(all.len(), 1);

        // A tagged row must not leak to another session even when target matches.
        let by_target = store
            .peek_for_targets(&["worker-1"], Some("other-session"), 10)
            .unwrap();
        assert_eq!(by_target.len(), 0);

        // Matching the session is not enough: the daemon must explicitly own
        // the target, otherwise stale session rows poison its LIMIT window.
        let by_session = store
            .peek_for_targets(&["nonexistent"], Some("my-session"), 10)
            .unwrap();
        assert!(by_session.is_empty());

        let owned = store
            .peek_for_targets(&["worker-1"], Some("my-session"), 10)
            .unwrap();
        assert_eq!(owned.len(), 1);
        assert_eq!(owned[0].factory_session.as_deref(), Some("my-session"));
    }

    #[test]
    fn poison_head_does_not_block_live_target_in_same_session() {
        let (_temp, store) = create_test_store();
        let poison = store
            .enqueue_with_session("supervisor", "dead-worker", "old poison", "factory-a")
            .unwrap();
        let live = store
            .enqueue_with_session("supervisor", "live-worker", "start work", "factory-a")
            .unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE prompt_queue SET created_at = ? WHERE id = ?",
                params![
                    (Utc::now()
                        - chrono::Duration::seconds(PROMPT_RETRY_MAX_AGE_SECS + 1))
                    .to_rfc3339(),
                    poison
                ],
            )
            .unwrap();
        }

        assert_eq!(
            store
                .abandon_ineligible_session_targets(
                    &["live-worker"],
                    "factory-a",
                    PROMPT_RETRY_MAX_AGE_SECS,
                )
                .unwrap(),
            1
        );

        let selected = store
            .peek_for_targets(&["live-worker"], Some("factory-a"), 10)
            .unwrap();
        assert_eq!(
            selected.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![live],
            "a stale session row for an unowned target must not occupy the live target's window"
        );
        assert!(!selected.iter().any(|row| row.id == poison));
        let poison_report = store.message_delivery_report(poison).unwrap().unwrap();
        assert_eq!(poison_report.stage, DeliveryStage::Abandoned);
        assert!(poison_report.delivered_at.is_none());
    }

    #[test]
    fn aged_pre_registration_wait_survives_first_gated_not_ready_delivery_pass() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "late-worker", "briefing", "factory-a")
            .unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE prompt_queue SET created_at = ? WHERE id = ?",
                params![
                    (Utc::now() - chrono::Duration::seconds(PROMPT_RETRY_MAX_AGE_SECS + 1))
                        .to_rfc3339(),
                    id
                ],
            )
            .unwrap();
        }

        // The target becomes eligible only when it registers into the daemon's
        // live target set. Its first delivery pass finds the pane startup gate
        // closed, before any transport attempt is made.
        assert_eq!(
            store
                .peek_for_targets(&["late-worker"], Some("factory-a"), 10)
                .unwrap()
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            vec![id]
        );
        store
            .record_pending_reason(
                id,
                PendingReason::GatedNotReady,
                Some("pane not ready for injection"),
            )
            .unwrap();

        let (attempts, first_attempt_at): (u32, Option<String>) = {
            let conn = store.conn.lock().unwrap();
            conn.query_row(
                "SELECT delivery_attempts, first_attempt_at
                 FROM prompt_queue WHERE id = ?",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap()
        };
        assert_eq!(
            attempts, 0,
            "pane readiness is a precondition, not a delivery attempt"
        );
        assert!(
            first_attempt_at.is_none(),
            "pre-registration queue age and readiness gating must not start retry age"
        );
        assert_eq!(
            store
                .peek_for_targets(&["late-worker"], Some("factory-a"), 10)
                .unwrap()
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            vec![id],
            "the aged briefing must remain deliverable after its first gated pass"
        );

        let first_real_failure = store
            .record_retry(
                id,
                PendingReason::TargetUnavailable,
                Some("transport unavailable"),
            )
            .unwrap();
        assert!(matches!(
            first_real_failure,
            PromptRetryDisposition::Scheduled { attempts: 1, .. }
        ));
        assert_ne!(
            store.message_delivery_report(id).unwrap().unwrap().stage,
            DeliveryStage::Abandoned,
            "created_at age must not terminally abandon the first real attempt"
        );
    }

    #[test]
    fn retry_age_exhaustion_is_measured_from_first_attempt() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "worker", "deliver me", "factory-a")
            .unwrap();
        let first = store
            .record_retry(id, PendingReason::TargetUnavailable, Some("pane missing"))
            .unwrap();
        assert!(matches!(
            first,
            PromptRetryDisposition::Scheduled { attempts: 1, .. }
        ));
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE prompt_queue SET first_attempt_at = ? WHERE id = ?",
                params![
                    (Utc::now() - chrono::Duration::seconds(PROMPT_RETRY_MAX_AGE_SECS + 1))
                        .to_rfc3339(),
                    id
                ],
            )
            .unwrap();
        }

        assert_eq!(
            store
                .record_retry(
                    id,
                    PendingReason::TargetUnavailable,
                    Some("still unavailable"),
                )
                .unwrap(),
            PromptRetryDisposition::Abandoned { attempts: 2 }
        );
    }

    #[test]
    fn retry_is_backed_off_then_permanently_terminal_after_bound() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "worker", "deliver me", "factory-a")
            .unwrap();

        let first = store
            .record_retry(id, PendingReason::TargetUnavailable, Some("pane missing"))
            .unwrap();
        assert!(matches!(
            first,
            PromptRetryDisposition::Scheduled { attempts: 1, .. }
        ));
        assert!(
            store
                .peek_for_targets(&["worker"], Some("factory-a"), 10)
                .unwrap()
                .is_empty(),
            "a failed row must not be selected again on the next 100ms daemon tick"
        );

        for _ in 1..PROMPT_RETRY_MAX_ATTEMPTS {
            store
                .record_retry(id, PendingReason::TargetUnavailable, Some("pane missing"))
                .unwrap();
        }

        assert!(
            store
                .peek_for_targets(&["worker"], Some("factory-a"), 10)
                .unwrap()
                .is_empty(),
            "an exhausted row must never become selectable again"
        );
        let report = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(report.stage, DeliveryStage::Abandoned);
        assert_eq!(report.legacy_status, MessageStatus::Delivered);
        assert!(report.delivered_at.is_none());
        assert_eq!(
            report.pending_reason,
            Some(PendingReason::AbandonedUnknownTarget)
        );
    }

    #[test]
    fn outstanding_for_targets_includes_rows_waiting_for_retry() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "worker", "deliver me", "factory-a")
            .unwrap();

        assert!(matches!(
            store
                .record_retry(id, PendingReason::TargetUnavailable, Some("pane missing"))
                .unwrap(),
            PromptRetryDisposition::Scheduled { attempts: 1, .. }
        ));
        assert!(
            store
                .peek_for_targets(&["worker"], Some("factory-a"), 10)
                .unwrap()
                .is_empty(),
            "delivery selection must honor retry backoff"
        );
        assert_eq!(
            store
                .outstanding_for_targets(&["worker"], Some("factory-a"))
                .unwrap()
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            vec![id],
            "observability must still see a queued row while delivery is backed off"
        );
    }

    #[test]
    fn explicit_age_remediation_abandons_only_old_pending_rows() {
        let (_temp, store) = create_test_store();
        let old = store.enqueue("supervisor", "dead-worker", "old").unwrap();
        let recent = store
            .enqueue("supervisor", "live-worker", "recent")
            .unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE prompt_queue SET created_at = ? WHERE id = ?",
                params![(Utc::now() - chrono::Duration::days(30)).to_rfc3339(), old],
            )
            .unwrap();
        }

        assert_eq!(store.abandon_pending_older_than(24 * 60 * 60).unwrap(), 1);
        assert_eq!(
            store.message_delivery_report(old).unwrap().unwrap().stage,
            DeliveryStage::Abandoned
        );
        assert_eq!(
            store
                .peek_for_targets(&["live-worker"], None, 10)
                .unwrap()
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>(),
            vec![recent]
        );
    }

    #[test]
    fn test_tagged_delivery_does_not_cross_sessions_on_name_collision() {
        let (_temp, store) = create_test_store();

        store
            .enqueue_with_session("supervisor-a", "worker", "session A", "session-a")
            .unwrap();
        store
            .enqueue_with_session("supervisor-b", "worker", "session B", "session-b")
            .unwrap();
        store
            .enqueue("legacy-supervisor", "worker", "legacy")
            .unwrap();

        let session_a = store
            .peek_for_targets(&["worker"], Some("session-a"), 10)
            .unwrap();
        assert_eq!(session_a.len(), 2);
        assert!(session_a.iter().any(|p| p.prompt == "session A"));
        assert!(session_a.iter().any(|p| p.prompt == "legacy"));
        assert!(!session_a.iter().any(|p| p.prompt == "session B"));

        let session_b = store
            .poll_for_target_with_session("worker", Some("session-b"), 10)
            .unwrap();
        assert_eq!(session_b.len(), 2);
        assert!(session_b.iter().any(|p| p.prompt == "session B"));
        assert!(session_b.iter().any(|p| p.prompt == "legacy"));
        assert!(!session_b.iter().any(|p| p.prompt == "session A"));

        let remaining = store.peek_all(10).unwrap();
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining[0].prompt, "session A");
    }

    #[test]
    fn test_priority_ordering() {
        let (_temp, store) = create_test_store();

        // Enqueue in reverse priority order: normal first, then critical
        store
            .enqueue_full(
                "supervisor",
                "worker",
                "Normal update",
                None,
                None,
                Some(NotificationPriority::Normal),
            )
            .unwrap();
        store
            .enqueue_full(
                "supervisor",
                "worker",
                "Critical blocker",
                None,
                None,
                Some(NotificationPriority::Critical),
            )
            .unwrap();
        store
            .enqueue_full(
                "supervisor",
                "worker",
                "High priority",
                None,
                None,
                Some(NotificationPriority::High),
            )
            .unwrap();

        let prompts = store.peek_all(10).unwrap();
        assert_eq!(prompts.len(), 3);
        // Critical (0) should come first, then High (1), then Normal (2)
        assert_eq!(prompts[0].prompt, "Critical blocker");
        assert_eq!(prompts[0].priority, NotificationPriority::Critical);
        assert_eq!(prompts[1].prompt, "High priority");
        assert_eq!(prompts[1].priority, NotificationPriority::High);
        assert_eq!(prompts[2].prompt, "Normal update");
        assert_eq!(prompts[2].priority, NotificationPriority::Normal);
    }

    #[test]
    fn test_default_priority_is_normal() {
        let (_temp, store) = create_test_store();

        store
            .enqueue("supervisor", "worker", "Default priority")
            .unwrap();

        let prompts = store.peek_all(10).unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].priority, NotificationPriority::Normal);
    }

    #[test]
    fn test_priority_with_peek_for_targets() {
        let (_temp, store) = create_test_store();

        store
            .enqueue_full(
                "worker",
                "supervisor",
                "Status update",
                Some("session-1"),
                None,
                Some(NotificationPriority::Normal),
            )
            .unwrap();
        store
            .enqueue_full(
                "worker",
                "supervisor",
                "BLOCKED: need help",
                Some("session-1"),
                None,
                Some(NotificationPriority::High),
            )
            .unwrap();

        let prompts = store
            .peek_for_targets(&["supervisor"], Some("session-1"), 10)
            .unwrap();
        assert_eq!(prompts.len(), 2);
        // High priority should come first
        assert_eq!(prompts[0].prompt, "BLOCKED: need help");
        assert_eq!(prompts[1].prompt, "Status update");
    }

    #[test]
    fn test_ack_delivery_confirmation() {
        let (_temp, store) = create_test_store();

        let id = store.enqueue("supervisor", "worker-1", "Do task").unwrap();

        // Initially pending
        let status = store.message_status(id).unwrap();
        assert_eq!(status, Some(MessageStatus::Pending));

        // Mark as processed (delivered)
        store.mark_processed(id).unwrap();
        let status = store.message_status(id).unwrap();
        assert_eq!(status, Some(MessageStatus::Delivered));

        // Ack (confirmed)
        store.ack(id).unwrap();
        let status = store.message_status(id).unwrap();
        assert_eq!(status, Some(MessageStatus::Confirmed));

        // Ack is idempotent
        store.ack(id).unwrap();

        // Peek shows acked_at is set
        let prompts = store.poll_for_target("worker-1", 10).unwrap();
        assert!(prompts.is_empty()); // already processed
    }

    #[test]
    fn test_ack_nonexistent_is_idempotent() {
        let (_temp, store) = create_test_store();
        // Acking a nonexistent prompt is idempotent — no error
        let result = store.ack(99999);
        assert!(result.is_ok());
    }

    #[test]
    fn test_message_status_nonexistent() {
        let (_temp, store) = create_test_store();
        let status = store.message_status(99999).unwrap();
        assert_eq!(status, None);
    }

    #[test]
    fn test_unacked_timeout() {
        let (_temp, store) = create_test_store();

        let id1 = store.enqueue("supervisor", "worker-1", "Msg 1").unwrap();
        let id2 = store.enqueue("supervisor", "worker-2", "Msg 2").unwrap();

        // Process both
        store.mark_processed(id1).unwrap();
        store.mark_processed(id2).unwrap();

        // Ack only one
        store.ack(id2).unwrap();

        // With timeout=0, all delivered-but-unacked messages should appear
        let unacked = store.unacked(0, 10).unwrap();
        assert_eq!(unacked.len(), 1);
        assert_eq!(unacked[0].id, id1);
        assert_eq!(unacked[0].prompt, "Msg 1");
    }

    #[test]
    fn test_unacked_respects_timeout() {
        let (_temp, store) = create_test_store();

        let id = store.enqueue("supervisor", "worker-1", "Recent").unwrap();
        store.mark_processed(id).unwrap();

        // With a large timeout, the recently processed message should NOT appear
        let unacked = store.unacked(3600, 10).unwrap();
        assert!(unacked.is_empty());
    }

    // ---- cas-c931: urgent (interrupt-and-redirect) flag ----

    #[test]
    fn test_enqueue_full_defaults_urgent_false() {
        let (_temp, store) = create_test_store();
        store
            .enqueue_full("supervisor", "worker", "Normal note", None, None, None)
            .unwrap();
        let prompts = store.peek_all(10).unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(
            !prompts[0].urgent,
            "non-urgent enqueue_full must default urgent=false"
        );
    }

    #[test]
    fn test_enqueue_urgent_roundtrips() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_urgent(
                "supervisor",
                "worker",
                "STOP — you are editing the wrong file",
                Some("sess-1"),
                Some("redirect"),
                Some(NotificationPriority::Critical),
                true,
            )
            .unwrap();
        assert!(id > 0);

        let prompts = store.peek_all(10).unwrap();
        assert_eq!(prompts.len(), 1);
        assert!(prompts[0].urgent, "urgent flag must round-trip as true");
        assert_eq!(prompts[0].priority, NotificationPriority::Critical);

        // Also visible via the session/target peek used by the daemon.
        let by_target = store
            .peek_for_targets(&["worker"], Some("sess-1"), 10)
            .unwrap();
        assert_eq!(by_target.len(), 1);
        assert!(by_target[0].urgent);
    }

    #[test]
    fn test_urgent_and_normal_coexist() {
        let (_temp, store) = create_test_store();
        store
            .enqueue_full("supervisor", "worker", "fyi", None, None, None)
            .unwrap();
        store
            .enqueue_urgent(
                "supervisor",
                "worker",
                "abort now",
                None,
                None,
                Some(NotificationPriority::Critical),
                true,
            )
            .unwrap();

        let prompts = store.poll_for_target("worker", 10).unwrap();
        assert_eq!(prompts.len(), 2);
        // Critical/urgent should sort ahead of the normal note.
        assert_eq!(prompts[0].prompt, "abort now");
        assert!(prompts[0].urgent);
        assert_eq!(prompts[1].prompt, "fyi");
        assert!(!prompts[1].urgent);
    }

    #[test]
    fn test_urgent_column_migration_on_legacy_table() {
        // Simulate a pre-cas-c931 prompt_queue table (no urgent column) and
        // confirm init() adds the column non-destructively and old rows read
        // back as urgent=false.
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("cas.db");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "CREATE TABLE prompt_queue (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    source TEXT NOT NULL,
                    target TEXT NOT NULL,
                    prompt TEXT NOT NULL,
                    created_at TEXT NOT NULL DEFAULT (datetime('now')),
                    processed_at TEXT,
                    factory_session TEXT,
                    summary TEXT,
                    priority INTEGER NOT NULL DEFAULT 2,
                    acked_at TEXT
                );",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO prompt_queue (source, target, prompt, created_at) VALUES ('s','w','legacy', datetime('now'))",
                [],
            )
            .unwrap();
        }

        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap(); // must add the urgent column without dropping the legacy row

        let prompts = store.peek_all(10).unwrap();
        assert_eq!(prompts.len(), 1);
        assert_eq!(prompts[0].prompt, "legacy");
        assert!(
            !prompts[0].urgent,
            "legacy rows must read back urgent=false"
        );

        // New urgent inserts work after migration.
        store
            .enqueue_urgent("s", "w", "new urgent", None, None, None, true)
            .unwrap();
        let prompts = store.poll_for_target("w", 10).unwrap();
        assert!(prompts.iter().any(|p| p.prompt == "new urgent" && p.urgent));
    }

    /// cas-2bcb / cas-04a6 R1: lower-ID NULL-session legacy rows must not
    /// occupy the fetch LIMIT ahead of eligible live-session rows.
    #[test]
    fn test_live_session_not_starved_by_legacy_null_session_hol() {
        let (_temp, store) = create_test_store();
        const LIMIT: usize = 10;

        for i in 0..(LIMIT + 5) {
            store
                .enqueue("old-worker", "supervisor", &format!("legacy backlog {i}"))
                .unwrap();
        }

        let live_id = store
            .enqueue_with_session(
                "worker-live",
                "supervisor",
                "live session coordination",
                "session-live",
            )
            .unwrap();

        let peeked = store
            .peek_for_targets(
                &["supervisor", "director", "all_workers", "worker-live"],
                Some("session-live"),
                LIMIT,
            )
            .unwrap();

        assert_eq!(
            peeked.len(),
            LIMIT,
            "peek must still respect the caller LIMIT"
        );
        assert!(
            peeked.iter().any(|p| p.id == live_id),
            "live-session row must appear in one peek despite >LIMIT lower-ID NULL-session backlog; got ids {:?}",
            peeked.iter().map(|p| p.id).collect::<Vec<_>>()
        );
        assert!(
            peeked.iter().any(|p| p.factory_session.is_none()),
            "legacy NULL-session rows remain eligible under the two-lane quota"
        );
    }

    /// Symmetric fairness: sustained live-session traffic must not starve
    /// eligible legacy NULL-session rows either (supervisor reject of pure
    /// session-first ordering).
    #[test]
    fn test_legacy_not_starved_by_live_session_traffic() {
        let (_temp, store) = create_test_store();
        const LIMIT: usize = 10;

        for i in 0..(LIMIT + 5) {
            store
                .enqueue_with_session(
                    "worker",
                    "supervisor",
                    &format!("live backlog {i}"),
                    "session-live",
                )
                .unwrap();
        }
        let legacy_id = store.enqueue("old", "supervisor", "lonely legacy").unwrap();

        let peeked = store
            .peek_for_targets(&["supervisor"], Some("session-live"), LIMIT)
            .unwrap();

        assert_eq!(peeked.len(), LIMIT);
        assert!(
            peeked.iter().any(|p| p.id == legacy_id),
            "legacy row must appear in one peek despite >LIMIT live-session backlog; got {:?}",
            peeked
                .iter()
                .map(|p| (p.id, p.factory_session.as_deref()))
                .collect::<Vec<_>>()
        );
        assert!(
            peeked
                .iter()
                .any(|p| p.factory_session.as_deref() == Some("session-live")),
            "session lane also represented"
        );
    }

    /// Repeated peek+mark batches: both lanes drain with bounded progress
    /// (neither lane stuck forever while the other has work).
    #[test]
    fn test_two_lane_bounded_progress_across_repeated_peeks() {
        let (_temp, store) = create_test_store();
        const LIMIT: usize = 10;
        const PER_LANE: usize = 25;

        let mut session_ids = Vec::new();
        let mut legacy_ids = Vec::new();
        for i in 0..PER_LANE {
            session_ids.push(
                store
                    .enqueue_with_session("w", "supervisor", &format!("sess {i}"), "session-a")
                    .unwrap(),
            );
            legacy_ids.push(
                store
                    .enqueue("old", "supervisor", &format!("leg {i}"))
                    .unwrap(),
            );
        }

        let mut session_seen = 0usize;
        let mut legacy_seen = 0usize;
        let mut rounds = 0usize;
        loop {
            let batch = store
                .peek_for_targets(&["supervisor"], Some("session-a"), LIMIT)
                .unwrap();
            if batch.is_empty() {
                break;
            }
            rounds += 1;
            assert!(rounds <= PER_LANE * 2, "must drain without infinite loop");

            let sess_in_batch = batch
                .iter()
                .filter(|p| p.factory_session.as_deref() == Some("session-a"))
                .count();
            let leg_in_batch = batch.iter().filter(|p| p.factory_session.is_none()).count();
            // While both lanes still have residual work beyond this batch,
            // each peek must take from both (bounded dual progress).
            let session_remaining_before = PER_LANE - session_seen;
            let legacy_remaining_before = PER_LANE - legacy_seen;
            if session_remaining_before > 0 && legacy_remaining_before > 0 && LIMIT >= 2 {
                assert!(
                    sess_in_batch >= 1 && leg_in_batch >= 1,
                    "round {rounds}: both lanes must progress while both pending; \
                     session={sess_in_batch} legacy={leg_in_batch}"
                );
            }
            session_seen += sess_in_batch;
            legacy_seen += leg_in_batch;

            // Within each lane contribution, FIFO by id.
            let sess_ids: Vec<i64> = batch
                .iter()
                .filter(|p| p.factory_session.as_deref() == Some("session-a"))
                .map(|p| p.id)
                .collect();
            assert!(
                sess_ids.windows(2).all(|w| w[0] < w[1]),
                "session lane FIFO violated: {sess_ids:?}"
            );
            let leg_ids: Vec<i64> = batch
                .iter()
                .filter(|p| p.factory_session.is_none())
                .map(|p| p.id)
                .collect();
            assert!(
                leg_ids.windows(2).all(|w| w[0] < w[1]),
                "legacy lane FIFO violated: {leg_ids:?}"
            );

            // Priority non-decreasing across the merged delivery set.
            for window in batch.windows(2) {
                assert!(window[0].priority as u8 <= window[1].priority as u8);
            }

            for p in &batch {
                store.mark_processed(p.id).unwrap();
            }
        }

        assert_eq!(session_seen, PER_LANE, "all session rows drained");
        assert_eq!(legacy_seen, PER_LANE, "all legacy rows drained");
    }

    /// Priority authoritative across lanes; FIFO within lane; isolation holds.
    #[test]
    fn test_peek_for_targets_priority_fifo_and_isolation_with_legacy() {
        let (_temp, store) = create_test_store();

        store
            .enqueue_with_session("sup-b", "worker", "other session", "session-b")
            .unwrap();

        let first = store
            .enqueue_with_session("worker-a", "supervisor", "live first", "session-a")
            .unwrap();
        let second = store
            .enqueue_with_session("worker-a", "supervisor", "live second", "session-a")
            .unwrap();
        let urgent = store
            .enqueue_urgent(
                "worker-a",
                "supervisor",
                "live urgent",
                Some("session-a"),
                Some("urgent"),
                Some(NotificationPriority::Critical),
                true,
            )
            .unwrap();

        for i in 0..15 {
            store
                .enqueue("old", "supervisor", &format!("legacy tail {i}"))
                .unwrap();
        }

        let peeked = store
            .peek_for_targets(&["supervisor", "worker"], Some("session-a"), 10)
            .unwrap();

        assert!(
            !peeked
                .iter()
                .any(|p| p.factory_session.as_deref() == Some("session-b")),
            "other session must not leak"
        );

        let live: Vec<_> = peeked
            .iter()
            .filter(|p| p.factory_session.as_deref() == Some("session-a"))
            .collect();
        assert_eq!(
            live.len(),
            3,
            "all three session-a rows must fit under quota"
        );
        assert_eq!(live[0].id, urgent, "urgent eligible precedes normal");
        assert_eq!(live[1].id, first, "equal-priority FIFO: first then second");
        assert_eq!(live[2].id, second);

        for window in peeked.windows(2) {
            assert!(
                window[0].priority as u8 <= window[1].priority as u8,
                "priority order violated: {:?} then {:?}",
                window[0].priority,
                window[1].priority
            );
        }
    }

    /// Pure merge helper: global priority before same-priority lane fairness.
    #[test]
    fn test_merge_two_lane_peeks_quota_contract() {
        fn row(id: i64, session: Option<&str>, priority: u8) -> QueuedPrompt {
            QueuedPrompt {
                id,
                source: "s".into(),
                target: "t".into(),
                prompt: format!("p{id}"),
                created_at: Utc::now(),
                processed_at: None,
                factory_session: session.map(|s| s.to_string()),
                summary: None,
                priority: NotificationPriority::from(priority),
                acked_at: None,
                urgent: priority == 0,
            }
        }

        // Equal priority both lanes full, limit 10 → 5+5 lane fairness.
        let session: Vec<_> = (1..=20).map(|i| row(i, Some("s"), 2)).collect();
        let legacy: Vec<_> = (100..=119).map(|i| row(i, None, 2)).collect();
        let merged = SqlitePromptQueueStore::merge_two_lane_peeks(session, legacy, 10);
        assert_eq!(merged.len(), 10);
        assert_eq!(
            merged
                .iter()
                .filter(|p| p.factory_session.is_some())
                .count(),
            5
        );
        assert_eq!(
            merged
                .iter()
                .filter(|p| p.factory_session.is_none())
                .count(),
            5
        );

        // One live + many legacy (same priority) → live included; remainder legacy.
        let session = vec![row(50, Some("s"), 2)];
        let legacy: Vec<_> = (1..=20).map(|i| row(i, None, 2)).collect();
        let merged = SqlitePromptQueueStore::merge_two_lane_peeks(session, legacy, 10);
        assert!(merged.iter().any(|p| p.id == 50));
        assert_eq!(merged.len(), 10);
        assert_eq!(
            merged
                .iter()
                .filter(|p| p.factory_session.is_none())
                .count(),
            9
        );

        // CRITICAL: session has 10 Critical, legacy 10 Normal, limit=10 →
        // all 10 Critical; zero Normal (global priority before lane quota).
        let session: Vec<_> = (1..=10).map(|i| row(i, Some("s"), 0)).collect();
        let legacy: Vec<_> = (100..=109).map(|i| row(i, None, 2)).collect();
        let merged = SqlitePromptQueueStore::merge_two_lane_peeks(session, legacy, 10);
        assert_eq!(merged.len(), 10);
        assert!(
            merged
                .iter()
                .all(|p| p.priority == NotificationPriority::Critical),
            "must not admit Normal while Critical remains; got {:?}",
            merged
                .iter()
                .map(|p| (p.id, p.priority as u8))
                .collect::<Vec<_>>()
        );

        // Symmetric: session 10 Normal, legacy 10 Critical → all Critical.
        let session: Vec<_> = (1..=10).map(|i| row(i, Some("s"), 2)).collect();
        let legacy: Vec<_> = (100..=109).map(|i| row(i, None, 0)).collect();
        let merged = SqlitePromptQueueStore::merge_two_lane_peeks(session, legacy, 10);
        assert_eq!(merged.len(), 10);
        assert!(
            merged
                .iter()
                .all(|p| p.priority == NotificationPriority::Critical)
        );

        // limit=1: Critical legacy beats Normal session.
        let session = vec![row(2, Some("s"), 2)];
        let legacy = vec![row(1, None, 0)];
        let merged = SqlitePromptQueueStore::merge_two_lane_peeks(session, legacy, 1);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, 1);
        assert_eq!(merged[0].priority, NotificationPriority::Critical);

        // limit=1 equal priority → session preferred (same-priority fairness).
        let session = vec![row(2, Some("s"), 2)];
        let legacy = vec![row(1, None, 2)];
        let merged = SqlitePromptQueueStore::merge_two_lane_peeks(session, legacy, 1);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].id, 2);
        assert!(merged[0].factory_session.is_some());
    }

    /// Store-level asymmetric priority + limit=1 regressions (supervisor gate).
    #[test]
    fn test_global_priority_before_lane_fairness_asymmetric_and_limit_one() {
        let (_temp, store) = create_test_store();

        // 10 Critical session + 10 Normal legacy → peek LIMIT 10 is all Critical.
        for i in 0..10 {
            store
                .enqueue_urgent(
                    "w",
                    "supervisor",
                    &format!("crit-sess {i}"),
                    Some("session-p"),
                    None,
                    Some(NotificationPriority::Critical),
                    true,
                )
                .unwrap();
        }
        for i in 0..10 {
            store
                .enqueue("old", "supervisor", &format!("norm-leg {i}"))
                .unwrap();
        }
        let peeked = store
            .peek_for_targets(&["supervisor"], Some("session-p"), 10)
            .unwrap();
        assert_eq!(peeked.len(), 10);
        assert!(
            peeked
                .iter()
                .all(|p| p.priority == NotificationPriority::Critical
                    && p.factory_session.as_deref() == Some("session-p")),
            "Critical session must fill LIMIT before any Normal legacy; got {:?}",
            peeked
                .iter()
                .map(|p| (p.priority as u8, p.factory_session.as_deref()))
                .collect::<Vec<_>>()
        );

        // Fresh store: Normal session flood + Critical legacy heads.
        let (_temp2, store2) = create_test_store();
        for i in 0..10 {
            store2
                .enqueue_with_session("w", "supervisor", &format!("norm {i}"), "session-p")
                .unwrap();
        }
        for i in 0..10 {
            store2
                .enqueue_urgent(
                    "old",
                    "supervisor",
                    &format!("crit-leg {i}"),
                    None,
                    None,
                    Some(NotificationPriority::Critical),
                    true,
                )
                .unwrap();
        }
        let peeked = store2
            .peek_for_targets(&["supervisor"], Some("session-p"), 10)
            .unwrap();
        assert_eq!(peeked.len(), 10);
        assert!(
            peeked.iter().all(
                |p| p.priority == NotificationPriority::Critical && p.factory_session.is_none()
            ),
            "Critical legacy must fill LIMIT before Normal session; got {:?}",
            peeked
                .iter()
                .map(|p| (p.priority as u8, p.factory_session.as_deref()))
                .collect::<Vec<_>>()
        );

        // limit=1: Critical legacy over Normal session.
        let (_temp3, store3) = create_test_store();
        store3
            .enqueue_with_session("w", "supervisor", "normal live", "session-p")
            .unwrap();
        let crit_id = store3
            .enqueue_urgent(
                "old",
                "supervisor",
                "critical legacy",
                None,
                None,
                Some(NotificationPriority::Critical),
                true,
            )
            .unwrap();
        let peeked = store3
            .peek_for_targets(&["supervisor"], Some("session-p"), 1)
            .unwrap();
        assert_eq!(peeked.len(), 1);
        assert_eq!(
            peeked[0].id, crit_id,
            "limit=1 must pick Critical legacy over Normal session"
        );
    }

    /// Scale + query plan: 10× backlog; EXPLAIN must name the expected indexes
    /// and must not use SCAN prompt_queue or USE TEMP B-TREE for the lane queries.
    #[test]
    fn test_two_lane_peek_scale_and_query_plan() {
        let (_temp, store) = create_test_store();
        const LIMIT: usize = 10;
        const BACKLOG: usize = LIMIT * 10;

        for i in 0..BACKLOG {
            store
                .enqueue_with_session("w", "supervisor", &format!("s{i}"), "session-scale")
                .unwrap();
            store
                .enqueue("old", "supervisor", &format!("l{i}"))
                .unwrap();
        }

        let started = std::time::Instant::now();
        let peeked = store
            .peek_for_targets(&["supervisor"], Some("session-scale"), LIMIT)
            .unwrap();
        let elapsed = started.elapsed();

        assert_eq!(peeked.len(), LIMIT);
        assert!(
            peeked
                .iter()
                .any(|p| p.factory_session.as_deref() == Some("session-scale"))
        );
        assert!(peeked.iter().any(|p| p.factory_session.is_none()));
        assert!(
            elapsed < std::time::Duration::from_millis(500),
            "peek over 10× backlog must stay fast; took {elapsed:?}"
        );

        fn explain_plan(
            conn: &rusqlite::Connection,
            sql: &str,
            params: &[&dyn rusqlite::ToSql],
        ) -> String {
            let mut plan = String::new();
            let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
            let rows = stmt
                .query_map(params, |row| Ok(row.get::<_, String>(3)?))
                .unwrap();
            for r in rows {
                plan.push_str(&r.unwrap());
                plan.push('\n');
            }
            plan
        }

        fn assert_index_plan(plan: &str, expected_index: &str, lane: &str) {
            let lower = plan.to_lowercase();
            assert!(
                plan.contains(expected_index),
                "{lane} plan must name {expected_index}; plan was:\n{plan}"
            );
            assert!(
                lower.contains("search") || lower.contains("using index"),
                "{lane} plan must SEARCH via index; plan was:\n{plan}"
            );
            // Reject full table scan of prompt_queue and temp sort materialization.
            assert!(
                !lower.contains("scan prompt_queue"),
                "{lane} plan must not SCAN prompt_queue; plan was:\n{plan}"
            );
            assert!(
                !lower.contains("use temp b-tree"),
                "{lane} plan must not USE TEMP B-TREE; plan was:\n{plan}"
            );
        }

        let conn = store.conn.lock().unwrap();
        let session_plan = explain_plan(
            &conn,
            "SELECT id FROM prompt_queue
             WHERE processed_at IS NULL AND factory_session = ?
             ORDER BY priority ASC, id ASC
             LIMIT ?",
            &[&"session-scale", &(LIMIT as i64)],
        );
        let legacy_plan = explain_plan(
            &conn,
            "SELECT id FROM prompt_queue
             WHERE processed_at IS NULL
               AND factory_session IS NULL
               AND target IN (?)
             ORDER BY priority ASC, id ASC
             LIMIT ?",
            &[&"supervisor", &(LIMIT as i64)],
        );

        assert_index_plan(&session_plan, "idx_prompt_queue_session_pending", "session");
        assert_index_plan(&legacy_plan, "idx_prompt_queue_legacy_pending", "legacy");
    }

    /// cas-2c5f: enqueued → selected → transport delivered → confirmed.
    /// mark_processed alone must NOT yield structured Delivered.
    #[test]
    fn test_message_delivery_report_stage_ladder_authoritative_delivery() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("sup", "worker", "hello", "sess-1")
            .unwrap();

        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.legacy_status, MessageStatus::Pending);
        assert_eq!(r.stage, DeliveryStage::Enqueued);
        assert_eq!(r.pending_reason, Some(PendingReason::AwaitingDelivery));
        assert_eq!(r.wake, ObservationStatus::Unobserved);
        assert_eq!(r.reaction, ObservationStatus::Unobserved);
        assert!(r.delivered_at.is_none());

        store.record_selected(id).unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::Selected);

        // Legacy mark_processed (queue drain) is NOT transport success.
        store.mark_processed(id).unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(
            r.legacy_status,
            MessageStatus::Delivered,
            "legacy ladder still uses processed_at"
        );
        assert_ne!(
            r.stage,
            DeliveryStage::Delivered,
            "structured Delivered requires transport_delivered_at"
        );
        assert!(r.delivered_at.is_none());

        // Authoritative handoff.
        store.mark_transport_delivered(id).unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::Delivered);
        assert_eq!(r.pending_reason, Some(PendingReason::AwaitingAck));
        assert!(r.delivered_at.is_some());

        store.ack(id).unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::Confirmed);
        assert_eq!(r.legacy_status, MessageStatus::Confirmed);
        assert!(r.pending_reason.is_none());
        assert_eq!(r.wake, ObservationStatus::Unobserved);
    }

    #[test]
    fn test_init_backfills_legacy_processed_rows_for_delivery_report() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("cas.db");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(super::PROMPT_QUEUE_SCHEMA).unwrap();
            conn.execute(
                "INSERT INTO prompt_queue (source, target, prompt, created_at, processed_at)
                 VALUES (?, ?, ?, ?, ?)",
                params![
                    "sup",
                    "worker",
                    "legacy hello",
                    "2026-07-21T12:00:00Z",
                    "2026-07-21T12:00:01Z"
                ],
            )
            .unwrap();
        }

        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();

        let r = store.message_delivery_report(1).unwrap().unwrap();
        assert_eq!(r.legacy_status, MessageStatus::Delivered);
        assert_eq!(r.stage, DeliveryStage::Delivered);
        assert_eq!(r.pending_reason, Some(PendingReason::AwaitingAck));
        assert_eq!(
            r.delivered_at.unwrap(),
            Utc.with_ymd_and_hms(2026, 7, 21, 12, 0, 1).unwrap()
        );
    }

    #[test]
    fn test_init_does_not_backfill_post_migration_mark_processed_rows() {
        let temp = TempDir::new().unwrap();
        let id = {
            let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
            store.init().unwrap();
            let id = store.enqueue("sup", "worker", "live legacy ack").unwrap();
            store.mark_processed(id).unwrap();
            id
        };

        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();

        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.legacy_status, MessageStatus::Delivered);
        assert_eq!(r.stage, DeliveryStage::Enqueued);
        assert_eq!(r.pending_reason, Some(PendingReason::AwaitingDelivery));
        assert!(r.delivered_at.is_none());

        let highest_stage: Option<String> = {
            let conn = store.conn.lock().unwrap();
            conn.query_row(
                "SELECT highest_stage FROM prompt_queue WHERE id = ?",
                params![id],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert!(highest_stage.is_none());
    }

    /// Gated must not regress to Selected when adapter failure is later stamped.
    #[test]
    fn test_message_delivery_report_stage_monotonic_gated_then_adapter() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("s", "worker", "body").unwrap();

        store
            .record_pending_reason(
                id,
                PendingReason::GatedNotReady,
                Some("pane not ready for injection"),
            )
            .unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::Gated);

        store
            .record_pending_reason(
                id,
                PendingReason::AdapterRetryable,
                Some("inject failed: broken pipe"),
            )
            .unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(
            r.stage,
            DeliveryStage::Gated,
            "stage must not regress Gated→Selected on adapter retry"
        );
        assert_eq!(r.pending_reason, Some(PendingReason::AdapterRetryable));
        assert!(r.delivered_at.is_none());
    }

    #[test]
    fn test_delivery_path_drop_suppress_abandon_never_transport_delivered() {
        let (_temp, store) = create_test_store();

        let drop_id = store.enqueue("dead-w", "supervisor", "from dead").unwrap();
        store
            .mark_dropped(drop_id, Some("source worker is dead"))
            .unwrap();
        let r = store.message_delivery_report(drop_id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::Dropped);
        assert_eq!(r.pending_reason, Some(PendingReason::DroppedDeadSource));
        assert!(r.delivered_at.is_none());
        assert_eq!(r.legacy_status, MessageStatus::Delivered); // processed_at set

        let sup_id = store.enqueue("w", "supervisor", "standing by").unwrap();
        store
            .mark_suppressed(sup_id, Some("duplicate idle"))
            .unwrap();
        let r = store.message_delivery_report(sup_id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::Suppressed);
        assert!(r.delivered_at.is_none());

        let ab_id = store.enqueue("s", "ghost-worker", "hi").unwrap();
        store
            .mark_abandoned(ab_id, Some("target not in session"))
            .unwrap();
        let r = store.message_delivery_report(ab_id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::Abandoned);
        assert!(r.delivered_at.is_none());
    }

    #[test]
    fn test_success_transport_delivery_stamps_delivered() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("sup", "worker", "do work").unwrap();
        store.record_selected(id).unwrap();
        store.mark_transport_delivered(id).unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::Delivered);
        assert!(r.delivered_at.is_some());
        assert_eq!(r.legacy_status, MessageStatus::Delivered);
        assert_eq!(r.pending_reason, Some(PendingReason::AwaitingAck));
    }

    /// cas-c061: historical delivered rows must not permanently reserve an
    /// exact message body. Once the recipient confirmed the first send, an
    /// intentional identical resend is a new queue event with a fresh ID.
    #[test]
    fn test_confirmed_worker_message_does_not_swallow_identical_resend() {
        let (_temp, store) = create_test_store();
        let first = store
            .enqueue_with_session("worker", "supervisor", "same report", "factory-a")
            .unwrap();
        store.mark_transport_delivered(first).unwrap();
        store.ack(first).unwrap();

        let second = store
            .enqueue_with_session("worker", "supervisor", "same report", "factory-a")
            .unwrap();

        assert_ne!(
            first, second,
            "a confirmed historical row must not impersonate a fresh enqueue"
        );
    }

    /// cas-c061: even without an explicit acknowledgement, exact-content
    /// collapse is bounded. A stale unconfirmed report is history, not a
    /// permanent reservation on that message body.
    #[test]
    fn test_stale_unconfirmed_worker_message_does_not_swallow_identical_resend() {
        let (_temp, store) = create_test_store();
        let first = store
            .enqueue_with_session("worker", "supervisor", "same report", "factory-a")
            .unwrap();
        store.mark_transport_delivered(first).unwrap();
        {
            let conn = store.conn.lock().unwrap();
            let stale_delivered_at =
                (Utc::now() - chrono::Duration::seconds(PROMPT_DUPLICATE_WINDOW_SECS + 1))
                    .to_rfc3339();
            conn.execute(
                "UPDATE prompt_queue SET transport_delivered_at = ? WHERE id = ?",
                params![stale_delivered_at, first],
            )
            .unwrap();
        }

        let second = store
            .enqueue_with_session("worker", "supervisor", "same report", "factory-a")
            .unwrap();
        assert_ne!(
            first, second,
            "an unconfirmed row beyond the dedup window must not swallow a resend"
        );
    }

    /// cas-c061: the enqueue-dedup and reciprocal-confirm predicates are hot
    /// on every coordination send. Their indexes must be installed only after
    /// the lifecycle columns have been migrated onto legacy prompt_queue DBs.
    #[test]
    fn test_message_send_hot_path_indexes_are_migrated() {
        let (_temp, store) = create_test_store();
        let conn = store.conn.lock().unwrap();
        for index in [
            "idx_prompt_queue_recent_unacked_dedupe",
            "idx_prompt_queue_ack_counterparty",
        ] {
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                     WHERE type = 'index' AND name = ?",
                    params![index],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(exists, 1, "missing migrated hot-path index {index}");
        }

        fn explain_plan(
            conn: &rusqlite::Connection,
            sql: &str,
            values: &[&dyn rusqlite::ToSql],
        ) -> String {
            let mut stmt = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {sql}"))
                .unwrap();
            stmt.query_map(rusqlite::params_from_iter(values.iter().copied()), |row| {
                row.get::<_, String>(3)
            })
            .unwrap()
            .map(|row| row.unwrap())
            .collect::<Vec<_>>()
            .join("\n")
        }

        let dedupe_plan = explain_plan(
            &conn,
            "SELECT id
             FROM prompt_queue
             WHERE source = ?
               AND target = ?
               AND prompt = ?
               AND factory_session IS ?
               AND urgent = 0
               AND transport_delivered_at IS NOT NULL
               AND acked_at IS NULL
               AND highest_stage IS NOT 'confirmed'
               AND transport_delivered_at >= ?
             ORDER BY transport_delivered_at DESC, id DESC
             LIMIT 1",
            &[
                &"worker",
                &"supervisor",
                &"body",
                &"factory-a",
                &"2026-07-29T00:00:00Z",
            ],
        );
        assert!(
            dedupe_plan.contains("idx_prompt_queue_recent_unacked_dedupe"),
            "dedupe query must use its migrated index; plan was:\n{dedupe_plan}"
        );
        assert!(
            !dedupe_plan.to_lowercase().contains("scan prompt_queue"),
            "dedupe query must not scan prompt_queue; plan was:\n{dedupe_plan}"
        );

        let ack_plan = explain_plan(
            &conn,
            "UPDATE prompt_queue
             SET acked_at = ?
             WHERE acked_at IS NULL
               AND transport_delivered_at IS NOT NULL
               AND target IN (?)
               AND source IN (?)
               AND (factory_session = ? OR factory_session IS NULL)",
            &[
                &"2026-07-29T00:00:01Z",
                &"worker",
                &"supervisor",
                &"factory-a",
            ],
        );
        assert!(
            ack_plan.contains("idx_prompt_queue_ack_counterparty"),
            "reciprocal-confirm query must use its migrated index; plan was:\n{ack_plan}"
        );
        assert!(
            !ack_plan.to_lowercase().contains("scan prompt_queue"),
            "reciprocal-confirm query must not scan prompt_queue; plan was:\n{ack_plan}"
        );
    }

    #[test]
    fn test_message_delivery_report_no_false_queue_head() {
        // Lower-id peer must not invent behind_queue_head (removed inaccurate heuristic).
        let (_temp, store) = create_test_store();
        let _first = store
            .enqueue_with_session("w", "supervisor", "first", "sess")
            .unwrap();
        let second = store
            .enqueue_with_session("w", "supervisor", "second", "sess")
            .unwrap();
        let r = store.message_delivery_report(second).unwrap().unwrap();
        assert_eq!(r.pending_reason, Some(PendingReason::AwaitingDelivery));
        assert_ne!(
            r.pending_reason.map(|p| p.as_str()),
            Some("behind_queue_head")
        );
    }

    #[test]
    fn test_message_delivery_report_unknown_id() {
        let (_temp, store) = create_test_store();
        assert!(store.message_delivery_report(999_999).unwrap().is_none());
        assert!(store.message_status(999_999).unwrap().is_none());
    }

    #[test]
    fn test_message_delivery_report_corrupt_created_at_errors() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("s", "w", "x").unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE prompt_queue SET created_at = 'not-a-timestamp' WHERE id = ?",
                params![id],
            )
            .unwrap();
        }
        let err = store.message_delivery_report(id).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("corrupt") || msg.contains("unparseable") || msg.contains("parse"),
            "expected parse error, got {msg}"
        );
    }

    #[test]
    fn test_broadcast_all_success_is_delivered() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("sup", "all_workers", "hi all").unwrap();
        store.mark_broadcast_outcome(id, 3, 3, 0, None).unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::Delivered);
        assert!(r.delivered_at.is_some());
        assert_eq!(r.broadcast_attempted, Some(3));
        assert_eq!(r.broadcast_succeeded, Some(3));
        assert_eq!(r.broadcast_failed, Some(0));
    }

    #[test]
    fn test_broadcast_mixed_is_partial_never_full_delivered() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("sup", "all_workers", "hi all").unwrap();
        store
            .mark_broadcast_outcome(id, 3, 2, 1, Some("w3 failed"))
            .unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::PartiallyDelivered);
        assert!(
            r.delivered_at.is_none(),
            "partial must not set transport_delivered_at"
        );
        assert_eq!(r.pending_reason, Some(PendingReason::PartialBroadcast));
        assert_eq!(r.broadcast_attempted, Some(3));
        assert_eq!(r.broadcast_succeeded, Some(2));
        assert_eq!(r.broadcast_failed, Some(1));
        // Legacy processed so queue drains (no re-inject of successes).
        assert_eq!(r.legacy_status, MessageStatus::Delivered);
    }

    #[test]
    fn test_broadcast_zero_success_stays_pending_retryable() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("sup", "all_workers", "hi all").unwrap();
        store
            .mark_broadcast_outcome(id, 2, 0, 2, Some("all failed"))
            .unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_ne!(r.stage, DeliveryStage::Delivered);
        assert!(r.delivered_at.is_none());
        assert_eq!(r.pending_reason, Some(PendingReason::AdapterRetryable));
        assert_eq!(r.legacy_status, MessageStatus::Pending); // not processed
    }

    #[test]
    fn test_broadcast_zero_intended_recipients() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("sup", "all_workers", "hi all").unwrap();
        store.mark_broadcast_outcome(id, 0, 0, 0, None).unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.pending_reason, Some(PendingReason::NoIntendedRecipients));
        assert!(r.delivered_at.is_none());
        assert_eq!(r.legacy_status, MessageStatus::Pending);
    }

    #[test]
    fn test_partial_to_delivered_legal_transition() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("sup", "all_workers", "hi").unwrap();
        store
            .mark_broadcast_outcome(id, 2, 1, 1, Some("mixed"))
            .unwrap();
        assert_eq!(
            store.message_delivery_report(id).unwrap().unwrap().stage,
            DeliveryStage::PartiallyDelivered
        );
        // Completing remaining recipients may advance Partial → Delivered.
        store.mark_transport_delivered(id).unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::Delivered);
        assert!(r.delivered_at.is_some());
    }

    #[test]
    fn test_illegal_terminal_sibling_rewrite_rejected() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("s", "w", "x").unwrap();
        store.mark_dropped(id, Some("dead")).unwrap();
        let err = store
            .mark_abandoned(id, Some("should fail"))
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("illegal stage transition"),
            "expected illegal transition, got {err}"
        );
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::Dropped);
        assert!(r.delivered_at.is_none());
    }

    #[test]
    fn test_corrupt_highest_stage_errors() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("s", "w", "x").unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE prompt_queue SET highest_stage = 'not-a-stage' WHERE id = ?",
                params![id],
            )
            .unwrap();
        }
        let err = store.message_delivery_report(id).unwrap_err().to_string();
        assert!(
            err.contains("corrupt") || err.contains("unknown highest_stage"),
            "got {err}"
        );
    }

    #[test]
    fn test_invariants_delivered_requires_transport_ts() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("s", "w", "x").unwrap();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE prompt_queue SET highest_stage = 'delivered', transport_delivered_at = NULL WHERE id = ?",
                params![id],
            )
            .unwrap();
        }
        let err = store.message_delivery_report(id).unwrap_err().to_string();
        assert!(err.contains("invariant violated"), "got {err}");
    }

    #[test]
    fn test_monotonic_repeated_calls_idempotent() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("s", "w", "x").unwrap();
        store.record_selected(id).unwrap();
        store.record_selected(id).unwrap();
        store
            .record_pending_reason(id, PendingReason::GatedNotReady, Some("gate"))
            .unwrap();
        store
            .record_pending_reason(id, PendingReason::GatedNotReady, Some("gate again"))
            .unwrap();
        store
            .record_pending_reason(id, PendingReason::AdapterRetryable, Some("retry"))
            .unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::Gated);
        assert_eq!(r.pending_reason, Some(PendingReason::AdapterRetryable));
        store.mark_transport_delivered(id).unwrap();
        store.mark_transport_delivered(id).unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(r.stage, DeliveryStage::Delivered);
        assert!(r.delivered_at.is_some());
        assert_eq!(r.wake, ObservationStatus::Unobserved);
    }

    /// cas-2c5f review 3: two independent SQLite connections to the same DB.
    /// A holds BEGIN IMMEDIATE while stamping Delivered; B waits then attempts
    /// Dropped. B must not overwrite — final stage is Delivered.
    #[test]
    fn test_cross_connection_immediate_tx_prevents_stale_overwrite() {
        use crate::shared_db::ImmediateTx;
        use std::sync::{Arc, Barrier};
        use std::thread;
        use std::time::Duration;

        let temp = TempDir::new().unwrap();
        let path = temp.path().to_path_buf();
        let db_path = path.join("cas.db");

        let id = {
            let store = SqlitePromptQueueStore::open(&path).unwrap();
            store.init().unwrap();
            store.enqueue("s", "w", "race").unwrap()
        };

        let barrier = Arc::new(Barrier::new(2));
        let db_a = db_path.clone();
        let db_b = db_path.clone();
        let barrier_a = Arc::clone(&barrier);
        let barrier_b = Arc::clone(&barrier);

        // Connection A: hold IMMEDIATE lock, stamp Delivered, then commit.
        let t_a = thread::spawn(move || {
            let conn = rusqlite::Connection::open(&db_a).unwrap();
            conn.busy_timeout(Duration::from_secs(10)).unwrap();
            let tx = ImmediateTx::new(&conn).unwrap();
            // Hold the write lock before B starts.
            barrier_a.wait();
            thread::sleep(Duration::from_millis(80));
            SqlitePromptQueueStore::atomic_stage_stamp_in_tx(
                &tx,
                id,
                DeliveryStage::Delivered,
                AtomicStampOpts {
                    reason: None,
                    detail: None,
                    set_processed: true,
                    broadcast_attempted: None,
                    broadcast_succeeded: None,
                    broadcast_failed: None,
                },
            )
            .unwrap();
            tx.commit().unwrap();
        });

        // Connection B: waits for A's IMMEDIATE, then tries illegal Dropped.
        let t_b = thread::spawn(move || {
            let conn = rusqlite::Connection::open(&db_b).unwrap();
            conn.busy_timeout(Duration::from_secs(10)).unwrap();
            barrier_b.wait();
            // Blocks until A commits, then reads Delivered and rejects Dropped.
            SqlitePromptQueueStore::atomic_stage_stamp(
                &conn,
                id,
                DeliveryStage::Dropped,
                AtomicStampOpts {
                    reason: Some(PendingReason::DroppedDeadSource),
                    detail: Some("stale writer"),
                    set_processed: true,
                    broadcast_attempted: None,
                    broadcast_succeeded: None,
                    broadcast_failed: None,
                },
            )
        });

        t_a.join().expect("writer A panicked");
        let b_result = t_b.join().expect("writer B panicked");
        assert!(
            b_result.is_err(),
            "stale Dropped after Delivered must fail; got {b_result:?}"
        );
        let err = b_result.unwrap_err().to_string();
        assert!(
            err.contains("illegal stage transition"),
            "expected illegal transition, got {err}"
        );

        let store = SqlitePromptQueueStore::open(&path).unwrap();
        let r = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(
            r.stage,
            DeliveryStage::Delivered,
            "Delivered must survive interleaved Dropped attempt"
        );
        assert!(r.delivered_at.is_some());
        assert_eq!(r.wake, ObservationStatus::Unobserved);
    }

    /// cas-88d8: concurrent SqlitePromptQueueStore::init on a legacy DB all succeed.
    #[test]
    fn test_concurrent_init_on_legacy_prompt_queue() {
        use std::sync::{Arc, Barrier};
        let temp = TempDir::new().unwrap();
        let cas_dir = temp.path().to_path_buf();
        let db_path = cas_dir.join("cas.db");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                r#"
                PRAGMA journal_mode=WAL;
                CREATE TABLE prompt_queue (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    source TEXT NOT NULL,
                    target TEXT NOT NULL,
                    prompt TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    processed_at TEXT
                );
                INSERT INTO prompt_queue (source, target, prompt, created_at)
                VALUES ('s', 'w', 'legacy', '2026-01-01T00:00:00Z');
                "#,
            )
            .unwrap();
        }

        let barrier = Arc::new(Barrier::new(6));
        let handles: Vec<_> = (0..6)
            .map(|_| {
                let cas_dir = cas_dir.clone();
                let barrier = Arc::clone(&barrier);
                std::thread::spawn(move || {
                    barrier.wait();
                    let store = SqlitePromptQueueStore::open(&cas_dir).unwrap();
                    store.init()
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap().expect("concurrent init must succeed");
        }
        let store = SqlitePromptQueueStore::open(&cas_dir).unwrap();
        store.init().unwrap();
        assert_eq!(store.peek_all(10).unwrap().len(), 1);
        store
            .enqueue_idempotent(
                "lifecycle:9",
                "supervisor",
                "body",
                None,
                None,
                None,
                "lifecycle-outbox:9",
            )
            .unwrap();
        assert_eq!(store.pending_count().unwrap(), 2);
    }

    /// cas-3a47: upgrade pre-dedupe_key prompt_queue without losing rows; init idempotent.
    #[test]
    fn test_upgrade_from_legacy_prompt_queue_adds_dedupe_key() {
        let temp = TempDir::new().unwrap();
        let db_path = temp.path().join("cas.db");
        {
            let conn = rusqlite::Connection::open(&db_path).unwrap();
            conn.execute_batch(
                r#"
                CREATE TABLE prompt_queue (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    source TEXT NOT NULL,
                    target TEXT NOT NULL,
                    prompt TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    processed_at TEXT
                );
                INSERT INTO prompt_queue (source, target, prompt, created_at)
                VALUES ('supervisor', 'worker-a', 'hello legacy', '2026-01-01T00:00:00Z');
                "#,
            )
            .unwrap();
        }

        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        store.init().unwrap(); // idempotent

        let pending = store.peek_all(10).unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].prompt, "hello legacy");

        // Idempotent enqueue works on upgraded schema.
        let r1 = store
            .enqueue_idempotent(
                "lifecycle:1",
                "supervisor",
                "body",
                Some("sess"),
                Some("sum"),
                None,
                "lifecycle-outbox:1",
            )
            .unwrap();
        let r2 = store
            .enqueue_idempotent(
                "lifecycle:1",
                "supervisor",
                "body-again",
                Some("sess"),
                Some("sum"),
                None,
                "lifecycle-outbox:1",
            )
            .unwrap();
        match (r1, r2) {
            (
                EnqueueIdempotentResult::Created(id1),
                EnqueueIdempotentResult::AlreadyExists(id2),
            ) => assert_eq!(id1, id2),
            other => panic!("expected Created then AlreadyExists: {other:?}"),
        }
        // legacy + one lifecycle row
        assert_eq!(store.pending_count().unwrap(), 2);
    }
}
