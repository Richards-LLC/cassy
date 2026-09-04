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

use crate::recording_store::capture_message_event;
use crate::shared_db::ImmediateTx;
use crate::supervisor_queue_store::NotificationPriority;
use crate::{Result, StoreError};

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

/// A peer warning is for an immediate shared-resource collision, not a
/// general worker chat channel. Keep one sender from monopolizing a peer's
/// queue while still allowing several independent collision warnings.
pub const WORKER_PEER_MESSAGE_BURST_LIMIT: i64 = 5;
const WORKER_PEER_MESSAGE_BURST_WINDOW_SECS: i64 = 60;

/// Age after which an *undelivered* queue row is treated as stale and is
/// quarantined instead of delivered (cas-d047, GH #69).
///
/// A row only reaches this age if no recipient ever consumed it — the bounded
/// retry policy above already terminates rows whose delivery was *attempted*
/// and failed. What survives is the misaddressed case: a row addressed to a
/// name that no live agent holds, sitting until some future session happens to
/// spawn a worker with the same name and hands it a months-old instruction
/// from a different lane. 24h is comfortably longer than any legitimate
/// spawn-then-assign or paused-session gap, and far shorter than the 4.5-month
/// delivery that motivated this bound.
pub const PROMPT_QUEUE_STALE_TTL_SECS: i64 = 24 * 60 * 60;

/// Structural idempotency marker for the sender-side delivery-stalled notice.
///
/// This is intentionally not inferred from `source`: prompt sources are free
/// text supplied by callers, whereas a dedupe key identifies the watchdog row
/// that must not bounce itself.
const DELIVERY_STALLED_BOUNCE_DEDUPE_PREFIX: &str = "delivery-stalled:";

/// Rows the daemon terminally quarantined are not deliverable content.
const TERMINAL_NON_DELIVERY_STAGES: &str = "('dropped', 'suppressed', 'abandoned')";

/// `last_pending_detail` written when a recipient's own drain consumes a row.
/// Accurate for the inbox-poll path, which does not ack: the row really is
/// waiting on the recipient's `message_ack`.
const DRAIN_DELIVERED_DETAIL: &str = "consumed by recipient inbox poll";

/// `last_pending_detail` written when the turn-start hook surfaced and acked a
/// row (cas-aac2). The raw `prompt_queue` table is what the delivery-mining
/// analysis reads, so a hook-acked row must not describe itself as an
/// inbox-poll consumption still awaiting an ack it already holds.
const HOOK_SURFACED_CONFIRMED_DETAIL: &str =
    "acked by turn-start hook surfacing into the recipient's prompt";

/// Daemon selection must skip rows the addressed recipient already consumed
/// (cas-d047, GH #70).
///
/// Two independent consumption signals, both recorded *outside* the daemon's
/// own `processed_at` bookkeeping, used to leave a row selectable:
/// - the recipient drained it through `poll_unseen_for_recipient` (inbox poll),
/// - the recipient acknowledged it (`ack` / `ack_delivered_for_recipient`).
///
/// Either way, re-selecting the row re-writes it to the recipient's inbox and,
/// on the idle-nudge path, types it into the pane a second time — the exact
/// duplicate deliveries reported in GH #70.
///
/// `all_workers` is deliberately exempt: broadcast read state is per-recipient
/// (`prompt_queue_recipient_seen`), so one worker's drain must never hide the
/// row from peers the daemon still has to deliver it to.
const NOT_ALREADY_CONSUMED_SQL: &str =
    "AND (target = 'all_workers' OR (acked_at IS NULL AND NOT EXISTS (
                       SELECT 1 FROM prompt_queue_recipient_seen seen
                       WHERE seen.prompt_id = prompt_queue.id
                         AND seen.recipient = prompt_queue.target)))";

/// cas-ac7e (GH #130): when may an ack remove a message from the recipient's
/// unread inbox?
///
/// Only on evidence about THIS message from THIS recipient: a surfacing
/// receipt (`prompt_queue_recipient_seen`, written by the recipient's own
/// drain) or the recipient's own `message_ack`
/// (`acked_via = 'explicit_ack'`).
///
/// The `poll_unseen_for_recipient` predicate used to read plain
/// `q.acked_at IS NULL`, which treats EVERY ack path as read state. That is
/// how notification 7212 vanished: it was stamped `inferred_from_reply` 55s
/// after enqueue — an inference about the recipient having taken *a* turn,
/// not about this message being surfaced — and was therefore filtered out of
/// the supervisor's own full `inbox_poll` drain ten minutes later, despite
/// having no `seen` row and never having been rendered to anyone. An ack that
/// is not the recipient's claim about this message must not be able to erase
/// the message from the only view that would reveal it; a message that
/// re-appears is recoverable, a message that vanishes is not.
///
/// `all_workers` stays exempt for the same reason as
/// [`NOT_ALREADY_CONSUMED_SQL`]: broadcast read state is per-recipient.
///
/// Requires the `prompt_queue` table to be aliased `q`.
const UNSURFACED_UNLESS_EXPLICIT_ACK_SQL: &str = "AND (q.target = 'all_workers'
                      OR q.acked_at IS NULL
                      OR q.acked_via IS NULL
                      OR q.acked_via <> 'explicit_ack')";

/// cas-dcf2 (GH #390): may later activity be recorded as a weak, visibly
/// non-confirming indication that a delivered message might have been seen?
///
/// Reply-inference (cas-6ad2) is the only confirmation path factory prompts
/// actually exercise, and it was unconditional: ANY later message from the
/// recipient to the same counterparty confirmed EVERY transport-delivered row
/// between them. That silently marked messages `confirmed` while the recipient
/// had never been shown them — zeroing `undelivered_after` and disarming the
/// supervisor's escalation gate on a worker idling against a stale premise.
///
/// Two independent gates, both required:
///
/// 1. **Ordering.** The reply must have been enqueued after the message's
///    transport handoff. A reply composed before the message existed (or while
///    it was still crossing in flight) cannot be a response to it.
/// 2. **Surfacing receipt.** CAS must hold a record that the message's content
///    was actually put in front of the recipient at or before the reply — a
///    `prompt_queue_recipient_seen` row for (message, addressed target),
///    written by the recipient's own inbox drain. Transport handoff is a write
///    to a file or a pane; it is not evidence anyone read it.
///
/// Weak evidence is not "probably fine": even with both gates, this only
/// earns `assumed_seen`; it never produces `confirmed`, clears the
/// undelivered clock, or stops escalation. An explicit `message_ack` or the
/// per-message turn artifact is the strong path and does not come through here.
pub fn reply_confirms_delivered_message(
    transport_delivered_at: Option<DateTime<Utc>>,
    recipient_seen_at: Option<DateTime<Utc>>,
    reply_enqueued_at: DateTime<Utc>,
) -> bool {
    let Some(delivered_at) = transport_delivered_at else {
        // Never handed to a transport — nothing could have been consumed.
        return false;
    };
    if delivered_at > reply_enqueued_at {
        return false;
    }
    let Some(seen_at) = recipient_seen_at else {
        return false;
    };
    seen_at <= reply_enqueued_at
}

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
    /// Who CAS observed writing this row, as stamped at enqueue time
    /// (cas-d9a8). `None` means the row carries no stamp at all — a legacy
    /// row written before the columns existed, or a path that never went
    /// through a stamping enqueue. It is deliberately NOT folded into
    /// [`QueueOrigin::Unattributed`]: "nobody was authenticated" and "we never
    /// looked" are different facts, and only the reader should decide they
    /// deserve the same treatment.
    ///
    /// Never derived from [`Self::source`]. `source` is what the caller typed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<QueueOrigin>,
}

/// A supervisor lifecycle wake relay that reached a terminal stage without
/// ever being transported (cas-7787, GH #160).
///
/// One of these is always a factory-level failure: a worker was parked behind
/// supervisor action, CAS said so, and the message did not arrive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UndeliveredLifecycleRelay {
    /// `prompt_queue.id` of the relay that never landed.
    pub prompt_id: i64,
    /// `lifecycle-wake:{notification_id}` source marker.
    pub source: String,
    /// Intended recipient (`supervisor`).
    pub target: String,
    /// Row summary — `{transition}: {task_id} ({occurrence})`.
    pub summary: Option<String>,
    /// Original lifecycle envelope. Consumers use its typed task payload to
    /// distinguish actionable relays from task-free informational events.
    pub prompt: String,
    /// Terminal stage the row died at (suppressed / dropped / abandoned).
    pub stage: String,
    /// Recorded reason, when one was stamped.
    pub reason: Option<PendingReason>,
    /// Forensic detail explaining the termination.
    pub detail: Option<String>,
    /// Owning factory session, when the row carried one.
    pub factory_session: Option<String>,
    /// When the relay was enqueued.
    pub created_at: DateTime<Utc>,
    /// When it was terminated.
    pub processed_at: Option<DateTime<Utc>>,
}

/// A pending queue row that has spent real transport attempts (cas-94a1).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RetriedPrompt {
    pub prompt_id: i64,
    pub source: String,
    pub target: String,
    pub summary: Option<String>,
    /// Transport attempts spent so far — the counter this type exists to read.
    pub delivery_attempts: u32,
    /// Reason stamped by the most recent failed attempt.
    pub reason: Option<PendingReason>,
    /// When the first attempt was spent.
    pub first_attempt_at: Option<DateTime<Utc>>,
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

-- Per-recipient read state is deliberately separate from processed_at
-- (daemon transport) and acked_at (delivery confirmation). A broadcast has
-- one prompt_queue row but many recipients, so its read state cannot live on
-- that row without one worker hiding the message from every other worker.
-- `source` (cas-7a01, GH #155) records WHICH surfacing path wrote the receipt.
-- A row drained by `inbox_poll` was handed to a tool result the recipient
-- asked for; a row stamped `hook_surfaced` was injected into the recipient's
-- turn by the UserPromptSubmit hook. Both are surfacing receipts, but only
-- the second one proves the content entered a turn without the recipient
-- having to know it should look — which is the distinction GH #155 needed and
-- the evidence `message_status` now reports as an observed wake.
CREATE TABLE IF NOT EXISTS prompt_queue_recipient_seen (
    prompt_id INTEGER NOT NULL,
    recipient TEXT NOT NULL,
    seen_at TEXT NOT NULL,
    source TEXT,
    PRIMARY KEY (prompt_id, recipient)
);

CREATE INDEX IF NOT EXISTS idx_prompt_queue_recipient_seen_recipient
    ON prompt_queue_recipient_seen(recipient, prompt_id);

-- cas-ac7e (GH #130): the recipient-side counterpart of the row-level
-- `transport_delivered_at` stamp. `stage=delivered` used to be a claim the
-- WRITER made about itself with nothing on the recipient's side to
-- corroborate it, which is how notifications 7179/7181/7183 could read
-- `stage=delivered` in `message_status` while the recipient's own drain
-- showed no delivery record for them at all. Written in the same
-- transaction as the Delivered stage stamp, so a delivered direct row
-- always has one. `all_workers` is excluded: a broadcast's per-recipient
-- transport is stamped by `mark_broadcast_outcome`'s counts, not here.
CREATE TABLE IF NOT EXISTS prompt_queue_recipient_transport (
    prompt_id INTEGER NOT NULL,
    recipient TEXT NOT NULL,
    delivered_at TEXT NOT NULL,
    PRIMARY KEY (prompt_id, recipient)
);

CREATE INDEX IF NOT EXISTS idx_prompt_queue_recipient_transport_recipient
    ON prompt_queue_recipient_transport(recipient, prompt_id);
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

/// Structured sender attribution for Commander semantic messages. Kept on the
/// durable queue row so transport and recipient receipts retain the exact
/// device/operator identity supplied on the wire.
const PROMPT_QUEUE_ATTRIBUTION_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN attribution_json TEXT;
"#;

/// Server-stamped sender provenance (cas-d9a8).
///
/// `source` is caller-settable (`cas factory message --from …`, bridge
/// POST /message), and `attribution_json` above is caller-SUPPLIED — both state
/// what a sender claims. Neither can answer "who actually wrote this row",
/// which is the only question a pane-wake decision may safely turn on: a wake
/// is a PTY write into someone's terminal.
///
/// These two columns are written ONLY by CAS's own enqueue path, from an
/// already-authenticated caller. No request field reaches them. A route that
/// cannot attribute its caller leaves them NULL, and NULL never wakes, so the
/// unattributable paths degrade to today's inbox-only behaviour instead of
/// being trusted by default.
///
/// Nullable and additive on purpose: legacy rows carry NULL and are treated as
/// unattributed. There is deliberately no backfill — inventing provenance for
/// rows whose sender was never recorded is exactly the fiction this column
/// exists to prevent.
const PROMPT_QUEUE_ORIGIN_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN origin_agent_id TEXT;
"#;

/// Coarse class of the stamped origin (`registered_agent`, `daemon`,
/// `unattributed`). Kept beside the id so the wake gate can reason about the
/// class without a registry lookup on the hot path, while the id remains the
/// authority for role resolution.
const PROMPT_QUEUE_ORIGIN_KIND_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN origin_kind TEXT;
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

/// Who actually wrote a queue row, as established by CAS rather than claimed
/// by the sender (cas-d9a8).
///
/// Constructed only inside CAS's enqueue paths from an already-authenticated
/// caller. It is deliberately NOT parsed from, defaulted from, or cross-checked
/// against `source`, `attribution_json`, or any request body: the whole point
/// is that it cannot be influenced by what a caller types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum QueueOrigin {
    /// An agent whose identity was resolved from the authenticated session at
    /// enqueue time. `agent_id` is the registry id, not a display name, so a
    /// later role lookup cannot be spoofed by naming someone.
    RegisteredAgent { agent_id: String },
    /// CAS's own machinery (lifecycle push, daemon relays, orphan recovery).
    Daemon,
    /// A route that cannot attribute its caller — `cas factory message`, bridge
    /// POST /message. Recorded explicitly rather than left implicit so an
    /// operator reading a row can tell "nobody was authenticated" apart from
    /// "this predates the column".
    Unattributed,
}

impl QueueOrigin {
    /// Column value for `origin_kind`.
    pub fn kind_str(&self) -> &'static str {
        match self {
            Self::RegisteredAgent { .. } => "registered_agent",
            Self::Daemon => "daemon",
            Self::Unattributed => "unattributed",
        }
    }

    /// Column value for `origin_agent_id`, when there is one.
    pub fn agent_id(&self) -> Option<&str> {
        match self {
            Self::RegisteredAgent { agent_id } => Some(agent_id.as_str()),
            Self::Daemon | Self::Unattributed => None,
        }
    }

    /// Rebuild a stamp from its two stored columns.
    ///
    /// Returns `None` for an unstamped row (`origin_kind` NULL), and also for
    /// a `registered_agent` row whose `origin_agent_id` is missing: a class
    /// that claims an authenticated sender but cannot name one is corrupt, and
    /// the safe reading of corrupt provenance is "no provenance", never
    /// "close enough". An unrecognised kind — a row written by a newer CAS —
    /// is likewise `None` rather than optimistically trusted.
    pub fn from_columns(agent_id: Option<String>, kind: Option<&str>) -> Option<Self> {
        match kind? {
            "registered_agent" => agent_id.map(|agent_id| Self::RegisteredAgent { agent_id }),
            "daemon" => Some(Self::Daemon),
            "unattributed" => Some(Self::Unattributed),
            _ => None,
        }
    }

    /// Can a row from this origin be considered for a pane wake at all?
    ///
    /// This answers only "is the sender established", never "should this
    /// particular row wake" — the envelope class is a separate, second factor
    /// applied by the wake gate. Both must hold.
    pub fn is_attributed(&self) -> bool {
        matches!(self, Self::RegisteredAgent { .. } | Self::Daemon)
    }
}

/// Result of a normal queue enqueue that may collapse a recent worker resend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnqueueOutcome {
    /// New row inserted.
    Created(i64),
    /// A recent, delivered, still-unconfirmed identical worker report exists.
    SuppressedDuplicate(i64),
}

/// The two durable rows created for an authorized worker-to-worker warning.
///
/// The supervisor row is deliberately part of the same store transaction as
/// the recipient row: a peer route without supervisory visibility is not a
/// valid route.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkerPeerMessageEnqueue {
    pub recipient_id: i64,
    pub supervisor_copy_id: i64,
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
///   {Dropped|Suppressed|Abandoned|PartiallyDelivered|Delivered} < AssumedSeen < Confirmed
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
    /// CAS observed later recipient activity after a per-message receipt, but
    /// has no transcript-level evidence that THIS message entered that turn.
    /// This is useful context, never an acknowledgement.
    AssumedSeen,
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
            Self::AssumedSeen => 4,
            Self::Confirmed => 5,
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
            Self::AssumedSeen => "assumed_seen",
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
            "assumed_seen" => Some(Self::AssumedSeen),
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
    /// The wake gate repeatedly declined while the recipient remained busy.
    /// The row was surfaced as undelivered rather than waiting for silence
    /// forever.
    UndeliveredAfterWakeDeclines,
    /// Terminal non-delivery: dead worker source dropped.
    DroppedDeadSource,
    /// Terminal non-delivery: duplicate idle suppression.
    SuppressedIdle,
    /// Terminal non-delivery by explicit dead-letter: the payload's premise
    /// expired before transport, so it was withdrawn rather than delivered as
    /// an instruction that is no longer true (cas-0147, GH #167).
    ///
    /// Split out of [`Self::SuppressedIdle`] because the conflation hid a
    /// four-day outage. Every one of the 397 `suppressed_idle` rows in the
    /// live queue came from premise expiry, not from idle chatter — 353 of
    /// them were supervisor lifecycle relays (34 of 36 `task_awaiting_merge`,
    /// 34 of 36 `task_close_rejected`) killed by an unpassable staleness test.
    /// Read as "idle suppression" the whole class looked benign and quiet by
    /// design, which is exactly why nobody went looking. A withdrawal is a
    /// decision someone made about a payload; it must not share a name with
    /// noise reduction.
    SupersededStale,
    /// Terminal non-delivery: unknown/stale target abandoned.
    AbandonedUnknownTarget,
    /// Terminal non-delivery of a supervisor lifecycle WAKE relay that was
    /// never transported (cas-7787, GH #160).
    ///
    /// Distinct from [`Self::SuppressedIdle`] on purpose. `SuppressedIdle`
    /// means "we withheld a copy the recipient did not need" — benign, and
    /// correctly quiet. This means "the factory told the supervisor a lane was
    /// parked behind them, and the supervisor never got it." In the reported
    /// session that difference was invisible: four `task_awaiting_merge`
    /// relays were stamped `suppressed_idle` with `transport_delivered_at`
    /// NULL, and nothing anywhere said a delivery had failed, so a human
    /// became the transport for three finished lanes. A relay that dies
    /// undelivered is a failure and must read as one.
    UndeliveredLifecycleRelay,
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
            Self::UndeliveredAfterWakeDeclines => "undelivered_after_wake_declines",
            Self::DroppedDeadSource => "dropped_dead_source",
            Self::SuppressedIdle => "suppressed_idle",
            Self::SupersededStale => "superseded_stale",
            Self::AbandonedUnknownTarget => "abandoned_unknown_target",
            Self::UndeliveredLifecycleRelay => "undelivered_lifecycle_relay",
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
            "undelivered_after_wake_declines" => Some(Self::UndeliveredAfterWakeDeclines),
            "dropped_dead_source" => Some(Self::DroppedDeadSource),
            "suppressed_idle" => Some(Self::SuppressedIdle),
            "superseded_stale" => Some(Self::SupersededStale),
            "abandoned_unknown_target" => Some(Self::AbandonedUnknownTarget),
            "undelivered_lifecycle_relay" => Some(Self::UndeliveredLifecycleRelay),
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
            Self::SuppressedIdle | Self::SupersededStale => DeliveryStage::Suppressed,
            Self::AbandonedUnknownTarget
            | Self::UndeliveredLifecycleRelay
            | Self::UndeliveredAfterWakeDeclines => {
                DeliveryStage::Abandoned
            }
            Self::PartialBroadcast => DeliveryStage::PartiallyDelivered,
            Self::NoIntendedRecipients => DeliveryStage::Selected,
            Self::AdapterRetryable
            | Self::AwaitingDelivery
            | Self::SessionIneligible
            | Self::AwaitingAck => DeliveryStage::Selected,
        }
    }

    /// Whether stamping this reason means a real transport attempt was spent
    /// (cas-94a1, GH #169).
    ///
    /// `delivery_attempts` sat at 0 across all 8,017 rows of the live queue —
    /// not because nothing incremented it, but because the only writer
    /// ([`PromptQueueStore::record_retry`]) is wired to four rare error
    /// branches this fleet has never taken, while the loop that actually
    /// retries a message dozens of times counted in a daemon-local `HashMap`
    /// that dies with the process. This classifier is what connects the
    /// durable column to the routine path.
    ///
    /// The distinction is load-bearing, not cosmetic. cas-d732/cas-7787
    /// established that a policy withhold "is withheld by policy, not a failed
    /// attempt — it must not burn the row's retry budget", so a blanket
    /// increment on every pending stamp would silently break an invariant
    /// another lane depends on. An attempt is spent only when the daemon
    /// handed the row to a transport and the transport did not take it.
    pub fn counts_as_delivery_attempt(self) -> bool {
        match self {
            // Transport was engaged and refused/failed the handoff.
            Self::AdapterRetryable | Self::TargetUnavailable => true,
            // Withheld before any transport was engaged — policy, cadence,
            // routing, or an audience that does not exist. No attempt spent.
            Self::GatedNotReady
            | Self::SessionIneligible
            | Self::AwaitingDelivery
            | Self::NoIntendedRecipients => false,
            // cas-94a1 decided against the POST-cas-78d3 machine, not the
            // pre-fix corpse data: now that hook surfacing really acks, a row
            // sitting in AwaitingAck has already been transported once. The
            // attempt that got it there is counted by whoever transported it;
            // waiting for the reply is not a second attempt.
            Self::AwaitingAck => false,
            // Terminal outcomes. The attempt that failed was counted when it
            // failed; the terminal stamp must not double-count it.
            Self::DroppedDeadSource
            | Self::SuppressedIdle
            | Self::SupersededStale
            | Self::AbandonedUnknownTarget
            | Self::UndeliveredLifecycleRelay
            | Self::UndeliveredAfterWakeDeclines
            | Self::PartialBroadcast => false,
        }
    }
}

impl std::fmt::Display for PendingReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Harness observation backed by a concrete external artifact.
///
/// `Observed` is never derived from queue age, delivery age, heartbeat age,
/// or any other elapsed-time heuristic. Callers may set it only while also
/// attaching the artifact timestamp and provenance to the corresponding
/// `*_observed_at` / `*_evidence` fields on [`MessageDeliveryReport`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStatus {
    /// No authoritative CAS observation for this stage.
    #[default]
    Unobserved,
    /// A concrete harness artifact records this stage.
    Observed,
}

impl std::fmt::Display for ObservationStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unobserved => write!(f, "unobserved"),
            Self::Observed => write!(f, "observed"),
        }
    }
}
/// Which surfacing path wrote a `prompt_queue_recipient_seen` receipt
/// (cas-7a01, GH #155).
///
/// Both values are genuine surfacing receipts — the row's content was put in
/// front of the recipient — but they are not the same evidence. `InboxPoll`
/// requires the recipient to have decided to look; `HookSurfaced` means CAS
/// injected the content into the recipient's turn at turn start, which is the
/// only path that can rescue a message the recipient does not know exists.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SurfacingSource {
    /// The recipient's own `inbox_poll` drain (cas-2816 / cas-d047 path).
    InboxPoll,
    /// The `UserPromptSubmit` hook injected the row into the recipient's turn.
    HookSurfaced,
    /// The daemon's own transport (agent-teams inbox file or PTY injection)
    /// put this row's content in front of this recipient (cas-b8ce, GH #176).
    ///
    /// WHY THIS VARIANT EXISTS: the receipt table used to be written by CAS's
    /// two surfacing paths ONLY. Every message a Claude teammate actually
    /// receives arrives over a different transport — `write_to_inbox` into the
    /// agent-teams inbox file, or a PTY injection — and those stamped
    /// `transport_delivered_at` while leaving the per-recipient receipt empty.
    /// `poll_unseen_for_recipient` defines "unread" as "no receipt", so the
    /// recipient's own `inbox_poll` re-served its entire already-actioned
    /// history. Two transports, one ledger.
    TransportDelivered,
}

impl SurfacingSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InboxPoll => "inbox_poll",
            Self::HookSurfaced => "hook_surfaced",
            Self::TransportDelivered => "transport_delivered",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "inbox_poll" => Some(Self::InboxPoll),
            "hook_surfaced" => Some(Self::HookSurfaced),
            "transport_delivered" => Some(Self::TransportDelivered),
            _ => None,
        }
    }
}

impl std::fmt::Display for SurfacingSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// What the daemon's wake nudge actually did for this row (cas-7a01, GH #155).
///
/// The non-urgent delivery path already computes all three states and used to
/// throw every one of them away: `pty_nudge` returns `InjectOutcome::Delivered`
/// from its success arm, its composer-deferred arm AND its error arm, so a
/// fired nudge, a vetoed nudge
/// and a failed nudge were indistinguishable at every caller. That is why
/// three separate incidents (GH #139, #155) could not tell "the nudge never
/// fired" from "the nudge fired but the harness started a turn without
/// surfacing the message" — the blanket `wake: unobserved` in
/// [`MessageDeliveryReport`] carried no information at all.
///
/// This is deliberately about CAS's own action, not about the harness: a
/// `Fired` nudge is proof CAS typed into the pane, never proof the recipient
/// read anything. Recipient-side evidence stays in
/// [`MessageDeliveryReport::wake`], which is only raised by a concrete
/// surfacing receipt.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum WakeAttempt {
    /// No wake was attempted for this row: the idle gate vetoed it, the
    /// recipient's channel is PTY (delivery already *is* a turn), or the row
    /// never reached the nudge seam. The default for every historical row.
    #[default]
    NotAttempted,
    /// CAS injected the wake into the recipient's pane and the mux reported
    /// the write as delivered.
    Fired,
    /// A wake was attempted and did not land — the inject errored, or it was
    /// deferred because the operator composer was dirty.
    Failed,
}

impl WakeAttempt {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotAttempted => "nudge_not_attempted",
            Self::Fired => "nudge_fired",
            Self::Failed => "nudge_failed",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "nudge_not_attempted" => Some(Self::NotAttempted),
            "nudge_fired" => Some(Self::Fired),
            "nudge_failed" => Some(Self::Failed),
            _ => None,
        }
    }

    /// Read a stored column value. An absent column is `NotAttempted` (no wake
    /// was ever recorded); an unrecognised value is also `NotAttempted` rather
    /// than an error, because a corrupt observability field must never make a
    /// delivery report unreadable.
    pub fn from_column(raw: Option<&str>) -> Self {
        raw.and_then(Self::parse).unwrap_or_default()
    }
}

impl std::fmt::Display for WakeAttempt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// Provenance of a message's `acked_at` stamp (cas-45c4 / GH #102).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfirmationSource {
    /// No ack recorded.
    Unconfirmed,
    /// The recipient called `message_ack` for this message id — its own claim
    /// about this specific message.
    ExplicitAck,
    /// Legacy provenance for a historical reply-inferred acknowledgement.
    /// New writes are demoted to [`DeliveryStage::AssumedSeen`] instead, so
    /// this value is retained only to decode pre-upgrade rows safely.
    InferredFromReply,
    /// cas-7a01 (GH #155): the `UserPromptSubmit` hook injected this message's
    /// content into the recipient's turn and recorded the receipt
    /// synchronously, at injection time.
    ///
    /// Deliberately a distinct value rather than an overload of
    /// [`ConfirmationSource::ExplicitAck`]: this is CAS's own observation of
    /// the injection it performed, not the recipient's assertion that it read
    /// anything. It is nevertheless a claim about THIS message and THIS
    /// recipient — unlike [`ConfirmationSource::InferredFromReply`], which is
    /// only evidence that some turn happened.
    HookSurfaced,
    /// Ack recorded before provenance was tracked, or by an unknown path.
    Unknown,
}

impl ConfirmationSource {
    pub fn from_column(raw: Option<&str>, has_ack: bool) -> Self {
        match (raw, has_ack) {
            (_, false) => Self::Unconfirmed,
            (Some("explicit_ack"), true) => Self::ExplicitAck,
            (Some("inferred_from_reply"), true) => Self::InferredFromReply,
            (Some("hook_surfaced"), true) => Self::HookSurfaced,
            (_, true) => Self::Unknown,
        }
    }

    /// Whether this confirmation is the recipient's own claim about this
    /// message, rather than an inference from unrelated activity.
    ///
    /// `HookSurfaced` qualifies: it is a per-message, per-recipient record
    /// that this content entered that recipient's turn. `InferredFromReply`
    /// does not, and never did.
    pub fn is_recipient_claim(self) -> bool {
        matches!(self, Self::ExplicitAck | Self::HookSurfaced)
    }
}

impl std::fmt::Display for ConfirmationSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unconfirmed => write!(f, "unconfirmed"),
            Self::ExplicitAck => write!(f, "explicit_ack"),
            Self::InferredFromReply => write!(f, "inferred_from_reply"),
            Self::HookSurfaced => write!(f, "hook_surfaced"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Stage-based delivery report for one prompt_queue message (cas-2c5f).
///
/// Additive to legacy [`MessageStatus`]. The store returns wake/reaction as
/// [`ObservationStatus::Unobserved`]; harness-aware query surfaces may enrich
/// them only from concrete artifacts. They are never inferred from elapsed
/// time or transport/confirmation timestamps.
///
/// `delivered_at` is **only** set after full transport handoff
/// (`transport_delivered_at`). Partial broadcasts never populate it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MessageDeliveryReport {
    pub id: i64,
    /// Original queue payload used only to correlate a concrete harness input
    /// record. It is deliberately omitted from the public status response.
    #[serde(skip)]
    pub prompt: String,
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
    /// cas-ac7e (GH #130): recipient-side corroboration of `delivered_at` —
    /// the `prompt_queue_recipient_transport` stamp for (this row, its
    /// addressed target), written in the same transaction as the Delivered
    /// stage stamp. `delivered_at` alone is the writer's claim about itself;
    /// #130 reported `stage=delivered` on rows whose recipient side showed no
    /// delivery record at all. `None` on `all_workers` (per-recipient
    /// transport is the broadcast counts) and on rows delivered before this
    /// table existed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recipient_transport_at: Option<DateTime<Utc>>,
    pub confirmed_at: Option<DateTime<Utc>>,
    /// Later recipient activity inferred from a reply. Unlike `confirmed_at`,
    /// this is not evidence that this specific message reached that turn.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assumed_seen_at: Option<DateTime<Utc>>,
    /// cas-45c4 (GH #102): how `confirmed_at` was obtained. `Unconfirmed` when
    /// there is no ack at all. Without this, a reply-inferred ack and an
    /// explicit recipient acknowledgement are indistinguishable — and only one
    /// of them is the recipient's own claim about THIS message.
    pub confirmation_source: ConfirmationSource,
    /// Present when waiting/blocked, partial, or terminal non-delivery.
    pub pending_reason: Option<PendingReason>,
    /// Human-readable detail for the pending reason (error text, …).
    pub pending_detail: Option<String>,
    /// Broadcast recipient counts when applicable (`all_workers`).
    pub broadcast_attempted: Option<u32>,
    pub broadcast_succeeded: Option<u32>,
    pub broadcast_failed: Option<u32>,
    pub wake: ObservationStatus,
    /// cas-7a01 (GH #155): what CAS's own wake nudge did for this row.
    ///
    /// Independent of [`MessageDeliveryReport::wake`], and answers a different
    /// question. `wake_attempt` is CAS reporting on the action it took;
    /// `wake` is CAS reporting whether the recipient demonstrably received a
    /// turn carrying this content. A row can be `nudge_fired` + `unobserved`
    /// (CAS typed into the pane, harness never surfaced it — the GH #155
    /// failure) or `nudge_not_attempted` + `observed` (no nudge was warranted,
    /// the recipient's next turn surfaced it through the hook anyway).
    pub wake_attempt: WakeAttempt,
    /// Consecutive times the wake gate declined this row while its recipient
    /// remained busy. Durable across daemon restarts so a busy pane cannot
    /// reset its starvation budget by reconnecting.
    pub wake_gate_declines: u32,
    /// When the wake attempt recorded in `wake_attempt` was made.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_attempt_at: Option<DateTime<Utc>>,
    /// Why a `nudge_failed` failed, or which gate declined a nudge.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_attempt_detail: Option<String>,
    /// Timestamp carried by the concrete harness wake artifact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_observed_at: Option<DateTime<Utc>>,
    /// Human-readable path + record shape that proves the wake observation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wake_evidence: Option<String>,
    pub reaction: ObservationStatus,
    /// Timestamp carried by the concrete harness reaction artifact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reaction_observed_at: Option<DateTime<Utc>>,
    /// Human-readable path + record shape that proves the reaction observation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reaction_evidence: Option<String>,
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

/// cas-45c4 (GH #102): how `acked_at` was obtained. `acked_at` alone conflates
/// two very different claims — the recipient explicitly acknowledged this
/// message, versus CAS inferred consumption because the recipient later
/// replied to that counterparty. Reporting both as "confirmed" is what let
/// `message_status` claim a recipient confirmed content it may never have
/// surfaced.
const PROMPT_QUEUE_ACKED_VIA_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN acked_via TEXT;
"#;

/// cas-dcf2 (GH #390): store activity inference separately from acknowledgement
/// so unrelated outbound traffic can never clear the delivery clock.
const PROMPT_QUEUE_ASSUMED_SEEN_AT_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN assumed_seen_at TEXT;
"#;

/// cas-7a01 (GH #155): persist the wake outcome the daemon already computes.
///
/// Before this, `pty_nudge`'s three arms all returned `Delivered` and nothing
/// was written anywhere, so `message_status` could only ever say
/// `wake: unobserved` — a constant, not a measurement. Three incidents
/// produced no signal because the column that would have split
/// "the nudge never fired" from "the nudge fired and the turn started without
/// surfacing" did not exist.
const PROMPT_QUEUE_WAKE_ATTEMPT_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN wake_attempt TEXT;
"#;
const PROMPT_QUEUE_WAKE_ATTEMPT_AT_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN wake_attempt_at TEXT;
"#;
const PROMPT_QUEUE_WAKE_ATTEMPT_DETAIL_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN wake_attempt_detail TEXT;
"#;
const PROMPT_QUEUE_WAKE_GATE_DECLINES_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN wake_gate_declines INTEGER NOT NULL DEFAULT 0;
"#;

/// Sender-side delivery-stalled bounces are one-shot across daemon restarts.
const PROMPT_QUEUE_DELIVERY_STALLED_NOTIFIED_AT_MIGRATION: &str = r#"
ALTER TABLE prompt_queue ADD COLUMN delivery_stalled_notified_at TEXT;
"#;

/// cas-7a01 (GH #155): which surfacing path wrote a receipt. NULL on rows
/// receipted before this column existed — those all came from `inbox_poll`,
/// the only writer at the time, but they are left NULL rather than
/// back-filled so a historical receipt is never presented as evidence of a
/// hook surfacing that never happened.
const PROMPT_QUEUE_RECIPIENT_SEEN_SOURCE_MIGRATION: &str = r#"
ALTER TABLE prompt_queue_recipient_seen ADD COLUMN source TEXT;
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
                None,
            )?
            .id())
    }

    /// Queue with an observable result for recent exact-content suppression.
    ///
    /// Legacy callers can continue using [`PromptQueueStore::enqueue_urgent`]
    /// when they only need the row ID. User-facing send paths should use this
    /// method so a reused ID is never presented as a fresh enqueue.
    ///
    /// `origin` is the server-stamped sender provenance (cas-d9a8). `None`
    /// leaves the row unstamped, which is the correct value for any route that
    /// cannot establish who its caller actually is — an unstamped row keeps
    /// today's inbox-only behaviour. It must never be synthesised from
    /// `source`, which is what the caller typed.
    fn enqueue_urgent_with_outcome(
        &self,
        source: &str,
        target: &str,
        prompt: &str,
        factory_session: Option<&str>,
        summary: Option<&str>,
        priority: Option<NotificationPriority>,
        urgent: bool,
        origin: Option<&QueueOrigin>,
    ) -> Result<EnqueueOutcome>;

    /// Atomically enqueue a same-factory-session worker warning and a visible
    /// copy for its supervisor. The store enforces the bounded peer burst so
    /// every service surface shares the same durable policy.
    fn enqueue_worker_peer_with_supervisor_copy(
        &self,
        source: &str,
        recipient: &str,
        supervisor: &str,
        prompt: &str,
        supervisor_copy: &str,
        factory_session: &str,
        summary: Option<&str>,
        priority: Option<NotificationPriority>,
        urgent: bool,
        origin: Option<&QueueOrigin>,
    ) -> Result<WorkerPeerMessageEnqueue>;

    /// Queue a prompt with structured sender attribution persisted on the same
    /// durable row. Existing MCP senders pass `None`; Commander messages pass
    /// their explicit device/operator wire object.
    fn enqueue_attributed_urgent_with_outcome(
        &self,
        source: &str,
        target: &str,
        prompt: &str,
        factory_session: Option<&str>,
        summary: Option<&str>,
        priority: Option<NotificationPriority>,
        urgent: bool,
        attribution: Option<&serde_json::Value>,
        origin: Option<&QueueOrigin>,
    ) -> Result<EnqueueOutcome> {
        let _ = attribution;
        self.enqueue_urgent_with_outcome(
            source,
            target,
            prompt,
            factory_session,
            summary,
            priority,
            urgent,
            origin,
        )
    }

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
        origin: Option<&QueueOrigin>,
    ) -> Result<EnqueueIdempotentResult>;

    /// Find direct messages that are still unread past their priority threshold.
    fn delivery_stalled_candidates(
        &self,
        factory_session: &str,
        priority_threshold_secs: i64,
        normal_threshold_secs: i64,
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>>;

    /// Atomically enqueue the one durable sender bounce if the original row is
    /// still unread; returns its notification ID when it was (or already is)
    /// created. A read or ack that wins the race cancels the bounce.
    fn enqueue_delivery_stalled_bounce(
        &self,
        prompt_id: i64,
        factory_session: &str,
        notice: &str,
        summary: &str,
    ) -> Result<Option<i64>>;

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

    /// Pull unread messages for one recipient without consuming daemon
    /// transport delivery.
    ///
    /// Direct rows must be unacknowledged. Broadcast acknowledgment remains
    /// recipient-scoped: row-level `acked_at` never hides an `all_workers` row
    /// from peers that have not seen it. Each returned row is atomically
    /// marked seen for this recipient only.
    fn poll_unseen_for_recipient(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>>;

    /// Turn-start surfacing drain for the `UserPromptSubmit` hook (cas-7a01,
    /// GH #155).
    ///
    /// Same eligibility predicate as [`PromptQueueStore::poll_unseen_for_recipient`],
    /// with three differences that matter:
    ///
    /// 1. The receipt is stamped `source = 'hook_surfaced'`, so
    ///    `message_status` can tell "the recipient chose to look" from "CAS
    ///    put this in front of the recipient".
    /// 2. The row is also acked with `acked_via = 'hook_surfaced'`, because
    ///    injection into a turn is a per-message claim about this recipient —
    ///    the evidence class `inferred_from_reply` only pretended to be.
    /// 3. Selection and receipt happen in ONE transaction. That is the
    ///    GH #124 storm guard stated as an invariant rather than a policy: a
    ///    caller can never hold content whose receipt failed to persist, so a
    ///    retried turn either re-surfaces a row that was genuinely never
    ///    surfaced, or surfaces nothing. It can never duplicate content into a
    ///    turn that already received it.
    ///
    /// Returns the rows injected, in queue order. Callers MUST render every
    /// returned row: the receipt is already written when this returns.
    ///
    /// [`PromptQueueStore::poll_unseen_for_recipient`]: PromptQueueStore::poll_unseen_for_recipient
    fn surface_unseen_for_recipient(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>>;

    /// Atomically surface unread rows from a bounded set of senders.
    ///
    /// This source-filtered counterpart leaves unrelated daemon/director
    /// traffic unread. An empty source set performs no query or write.
    fn surface_unseen_from_sources_for_recipient(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
        sources: &[&str],
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>>;

    /// Record that a transport OTHER than CAS's own surfacing paths put this
    /// row's content in front of `recipient` (cas-b8ce, GH #176).
    ///
    /// # Why this exists
    ///
    /// `prompt_queue_recipient_seen` is the single ledger every "has this
    /// recipient read it" question is answered from — most importantly
    /// [`PromptQueueStore::poll_unseen_for_recipient`], whose predicate is
    /// literally `seen.prompt_id IS NULL`. Until this method existed, that
    /// ledger was written by two callers only: the `inbox_poll` drain and the
    /// `UserPromptSubmit` hook. But the transport that actually delivers to a
    /// Claude teammate is the agent-teams inbox file (`write_to_inbox`) or a
    /// PTY injection, and those recorded delivery in `prompt_queue` columns
    /// (`transport_delivered_at`, `processed_at`, `highest_stage`) that the
    /// unread predicate does not consult. So a message could be delivered,
    /// read, replied to and acted on, and still be re-served in full by the
    /// recipient's next `inbox_poll` — GH #176's redelivery bursts.
    ///
    /// # Contract
    ///
    /// Callers must hold POSITIVE per-message evidence that THIS recipient
    /// received THIS content — a completed PTY injection, or the harness
    /// having taken the inbox copy with the pane then producing output. A
    /// transport *attempt* is not evidence and must not call this: writing a
    /// receipt for content nobody saw makes the message vanish from the only
    /// view that would reveal it, which is the failure mode cas-ac7e
    /// (GH #130) exists to prevent.
    ///
    /// Deliberately does NOT set `acked_at`. Delivery is not acknowledgement;
    /// any later outbound activity is separately recorded as `assumed_seen`,
    /// never as a weaker acknowledgement. This closes the redelivery hole
    /// without letting unrelated activity clear confirmation.
    ///
    /// Idempotent (`INSERT OR IGNORE`): a re-observed delivery never moves an
    /// existing receipt's timestamp, so reply-inference ordering
    /// ([`reply_confirms_delivered_message`]) cannot be retroactively broken.
    fn record_recipient_surfaced(
        &self,
        prompt_id: i64,
        recipient: &str,
        source: SurfacingSource,
    ) -> Result<()>;

    /// Record what the daemon's wake nudge did for this row (cas-7a01).
    ///
    /// Best-effort observability: callers should not fail a delivery because
    /// the wake outcome could not be persisted. Writing `NotAttempted` over an
    /// existing `Fired` is rejected so a later, unrelated pass cannot erase the
    /// evidence that a wake was once sent.
    fn record_wake_attempt(
        &self,
        prompt_id: i64,
        attempt: WakeAttempt,
        detail: Option<&str>,
    ) -> Result<()>;

    /// Persist one declined wake-gate pass and return its consecutive count.
    /// The count is per message, not per daemon process.
    fn record_wake_gate_decline(&self, prompt_id: i64, detail: &str) -> Result<u32>;

    /// Count the messages `recipient` has NOT yet seen, without consuming them.
    ///
    /// cas-e728 (GH #105): `worker_status` needs to say whether a quiet worker
    /// has mail waiting, and a supervisor reading status must never mark that
    /// mail seen — so this shares `poll_unseen_for_recipient`'s predicate
    /// (recipient-seen state, stale/terminal-stage exclusion, `all_workers`
    /// fan-out) but takes no write and returns only a count.
    ///
    /// Deliberately NOT `processed_at IS NULL`: the daemon stamps
    /// `processed_at` the instant it hands a row to the transport, so that
    /// column answers "has the daemon ticked", not "has the worker read it".
    /// The whole point here is the row that WAS delivered and never consumed.
    fn count_unseen_for_recipient(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
    ) -> Result<usize>;

    /// The rows behind [`count_unseen_for_recipient`], without consuming them.
    ///
    /// cas-f08d (GH #147): a count alone cannot tell a work message from a
    /// fired-reminder delivery, and those two mean opposite things about a
    /// quiet worker — unconsumed work is a stall signal, an already-acted-on
    /// reminder is not. `worker_status` needs the prompt text and timestamps to
    /// classify them, so it reads the same rows the count is derived from.
    ///
    /// Read-only by construction, exactly like the count: a supervisor
    /// inspecting an inbox must never mark that inbox seen.
    ///
    /// [`count_unseen_for_recipient`]: PromptQueueStore::count_unseen_for_recipient
    fn peek_unseen_for_recipient(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>>;

    /// Age in seconds of the oldest message `recipient` has not seen.
    ///
    /// `None` when the recipient's inbox is empty. cas-e728 uses this to tell
    /// a worker that is merely between turns (no mail) from one that was handed
    /// work and never woke (old unseen mail) — the latter is a real stall on a
    /// harness whose turns CAS cannot observe.
    fn oldest_unseen_age_secs_for_recipient(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
    ) -> Result<Option<i64>>;

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

    /// Acknowledge the prompt outbox row identified by `dedupe_key`.
    ///
    /// Lifecycle envelopes expose their durable supervisor notification ID,
    /// while the daemon redelivers a separate prompt row. This lookup bridges
    /// those identities without assuming the two SQLite sequences coincide.
    /// Returns the linked prompt ID when present.
    fn ack_by_dedupe_key(&self, dedupe_key: &str) -> Result<Option<i64>>;

    /// Acknowledge a terminal lifecycle relay by the ID embedded in its
    /// `lifecycle-wake:<id>` source marker. These IDs are not prompt IDs.
    fn ack_lifecycle_wake(&self, lifecycle_wake_id: i64) -> Result<Option<i64>>;

    /// Rewrite a not-yet-terminal prompt before its first bounded delivery.
    ///
    /// Used to replace a family of task-free duplicate worker-death relays
    /// with one forensic batch. Delivered/acknowledged rows are never changed.
    fn rewrite_pending(&self, prompt_id: i64, prompt: &str, summary: Option<&str>) -> Result<bool>;

    /// Confirm delivered messages that the current recipient demonstrably
    /// consumed by sending a response back to one of `sender_aliases`.
    ///
    /// Factory supervisors have both a display name and the logical
    /// `"supervisor"` alias, so both sides are expressed as alias slices.
    /// Only transport-delivered, still-unacked rows in the observing factory
    /// session are advanced.
    ///
    /// cas-dcf2 (GH #390): record only the weaker fact that later recipient
    /// activity followed a per-message receipt. This MUST NOT confirm the
    /// message: confirmation requires explicit acknowledgement or a
    /// message-specific turn artifact.
    fn ack_delivered_for_recipient(
        &self,
        recipient_aliases: &[&str],
        sender_aliases: &[&str],
        factory_session: Option<&str>,
        reply_enqueued_at: DateTime<Utc>,
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
    ///
    /// Reserved for genuine noise reduction — a duplicate "standing by" the
    /// recipient does not need. A payload withdrawn because its premise
    /// expired is [`Self::mark_superseded`], not this (cas-0147, GH #167).
    fn mark_suppressed(&self, prompt_id: i64, detail: Option<&str>) -> Result<()>;

    /// Explicit dead-letter for a payload whose premise expired before
    /// transport (cas-0147, GH #167): processed, stage `Suppressed`, reason
    /// [`PendingReason::SupersededStale`].
    ///
    /// `detail` is mandatory here and not `Option` on purpose. A row may only
    /// reach a terminal non-delivered state by a dead-letter that says why;
    /// "it was withdrawn, reason unrecorded" is the state this whole class of
    /// bug hides in.
    fn mark_superseded(&self, prompt_id: i64, detail: &str) -> Result<()>;

    /// Unknown-target abandon: processed without transport success.
    fn mark_abandoned(&self, prompt_id: i64, detail: Option<&str>) -> Result<()>;

    /// Terminate a supervisor lifecycle WAKE relay that was never transported
    /// (cas-7787, GH #160): processed, stage `Abandoned`, reason
    /// [`PendingReason::UndeliveredLifecycleRelay`].
    ///
    /// The row still terminates — leaving it pending would re-write a payload
    /// whose premise has expired every re-nudge tick (the GH #124 storm). What
    /// changes is that it terminates as a recorded FAILURE instead of a benign
    /// suppression, so [`PromptQueueStore::list_undelivered_lifecycle_relays`]
    /// can surface it and the factory stops mistaking silence for success.
    fn mark_undelivered_lifecycle_relay(&self, prompt_id: i64, detail: Option<&str>) -> Result<()>;

    /// Terminate an ordinary direct message after the bounded wake gate has
    /// declined every re-offer. This is visibly distinct from a lifecycle
    /// relay, so `message_status` does not turn worker starvation into an
    /// unrelated supervisor-lifecycle diagnosis.
    fn mark_undelivered_after_wake_declines(
        &self,
        prompt_id: i64,
        detail: Option<&str>,
    ) -> Result<()>;

    /// Pending rows that have burned at least `min_attempts` transport
    /// attempts, worst first (cas-94a1, GH #169).
    ///
    /// The read side that makes `delivery_attempts` worth writing. A message
    /// the factory has tried and failed to hand over repeatedly is the earliest
    /// honest signal that a recipient is unreachable — available here before
    /// the row exhausts its budget and dies.
    fn list_most_retried_pending(
        &self,
        min_attempts: u32,
        limit: usize,
    ) -> Result<Vec<RetriedPrompt>>;

    /// Lifecycle wake relays that reached a terminal stage without ever being
    /// transported (cas-7787, GH #160).
    ///
    /// This is the failure-honesty read side: every row here is a moment the
    /// factory told the supervisor a lane was parked behind them and the
    /// supervisor never received it. Derived entirely from columns the queue
    /// already writes (`transport_delivered_at IS NULL` + a terminal
    /// `highest_stage` + a `lifecycle-wake:` source), so it reports historical
    /// incidents too, not only ones recorded after this code shipped.
    fn list_undelivered_lifecycle_relays(
        &self,
        limit: usize,
    ) -> Result<Vec<UndeliveredLifecycleRelay>>;

    /// Durably reconcile terminal lifecycle relays for tasks that can no
    /// longer need supervisor action.  The forensic prompt row and its final
    /// delivery stage are retained; only its replay eligibility is cleared.
    /// Returns the number of previously-unacknowledged rows reconciled.
    fn reconcile_terminal_lifecycle_relays(&self) -> Result<usize>;

    /// Count all unresolved terminal lifecycle relay rows without applying a
    /// display cap.  Status banners use this with a bounded sample so they
    /// cannot hide backlog depth.
    fn undelivered_lifecycle_relay_count(&self) -> Result<usize>;

    /// Get count of pending prompts
    fn pending_count(&self) -> Result<usize>;

    /// Terminally abandon pending prompts older than the requested age.
    ///
    /// This is the safe remediation primitive for historical poison queues:
    /// it preserves rows and their forensic status instead of deleting them.
    fn abandon_pending_older_than(&self, older_than_secs: i64) -> Result<usize>;

    /// Quarantine undelivered rows older than `older_than_secs` (cas-d047).
    ///
    /// Unlike [`PromptQueueStore::abandon_ineligible_session_targets`], this is
    /// not scoped to a roster: a stale row is stale whatever its target and
    /// whatever session tagged it (NULL-session rows are the ones that leaked
    /// across sessions in GH #69). Rows are marked `Abandoned` with a forensic
    /// detail — never deleted — and returned so the caller can log exactly what
    /// was withheld from delivery.
    fn expire_stale_pending(&self, older_than_secs: i64) -> Result<Vec<QueuedPrompt>>;

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
        // Columns 11/12 = the server-stamped origin (cas-d9a8). Tolerate
        // absence the same way `urgent` above does, so a SELECT that predates
        // the columns still parses; an absent stamp reads as `None`, which the
        // wake gate treats as "not established" rather than as permission.
        let origin_agent_id: Option<String> = row.get(11).unwrap_or(None);
        let origin_kind: Option<String> = row.get(12).unwrap_or(None);
        let origin = QueueOrigin::from_columns(origin_agent_id, origin_kind.as_deref());

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
            origin,
        })
    }

    fn record_retry(
        &self,
        prompt_id: i64,
        reason: PendingReason,
        detail: Option<&str>,
    ) -> Result<PromptRetryDisposition> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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

        // cas-ac7e (GH #130): a Delivered stamp must leave a recipient-side
        // record, not just a writer-side column. Same transaction, so the two
        // truths cannot diverge: if `message_status` reports stage=delivered
        // for a direct row, `prompt_queue_recipient_transport` holds the
        // matching (row, addressed recipient) stamp. INSERT OR IGNORE keeps
        // re-stamping idempotent and preserves the FIRST handoff instant,
        // mirroring the COALESCE on `transport_delivered_at` above.
        if stamp_transport {
            tx.execute(
                "INSERT OR IGNORE INTO prompt_queue_recipient_transport
                     (prompt_id, recipient, delivered_at)
                 SELECT id, target, ?
                   FROM prompt_queue
                  WHERE id = ? AND target <> 'all_workers'",
                params![now, prompt_id],
            )?;
        }
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

    /// Not pending on anything, but with a detail recording *why* the stage
    /// moved (cas-aac2). Reason stays `None` so no reader mistakes an
    /// already-settled row for one still waiting on something.
    fn detail(detail: &'a str) -> Self {
        Self {
            reason: None,
            detail: Some(detail),
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

/// cas-e728 (GH #105): shared read-only evaluation of a recipient's unseen
/// inbox — count plus the age of its oldest row. Mirrors
/// `poll_unseen_for_recipient`'s predicate exactly (recipient-seen state,
/// stale/terminal-stage exclusion, `all_workers` fan-out) so status can never
/// disagree with what the recipient's own next poll would hand it.
impl SqlitePromptQueueStore {
    /// The FROM + WHERE half of the unseen-inbox predicate, plus its bound
    /// parameters in order. Shared verbatim by the count/age summary and the
    /// row-level peek so the two can never drift apart — a peek that returned
    /// rows the summary did not count (or vice versa) would put `worker_status`
    /// at odds with itself.
    fn unseen_for_recipient_predicate(
        recipient: &str,
        factory_session: Option<&str>,
    ) -> (String, Vec<Box<dyn rusqlite::ToSql>>) {
        let stale_cutoff =
            (Utc::now() - chrono::Duration::seconds(PROMPT_QUEUE_STALE_TTL_SECS)).to_rfc3339();
        let deliverable_sql = format!(
            "AND q.created_at >= ?
             AND (q.highest_stage IS NULL
                  OR q.highest_stage NOT IN {TERMINAL_NON_DELIVERY_STAGES})"
        );
        let session_sql = if factory_session.is_some() {
            "AND (q.factory_session = ? OR q.factory_session IS NULL)"
        } else {
            "AND q.factory_session IS NULL"
        };
        let sql = format!(
            "FROM prompt_queue q
             LEFT JOIN prompt_queue_recipient_seen seen
               ON seen.prompt_id = q.id AND seen.recipient = ?
             WHERE seen.prompt_id IS NULL
               {UNSURFACED_UNLESS_EXPLICIT_ACK_SQL}
               {deliverable_sql}
               AND (q.target = ? OR q.target = 'all_workers')
               {session_sql}"
        );
        let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(recipient.to_string()),
            Box::new(stale_cutoff),
            Box::new(recipient.to_string()),
        ];
        if let Some(session) = factory_session {
            params.push(Box::new(session.to_string()));
        }
        (sql, params)
    }

    fn unseen_for_recipient_summary(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
    ) -> Result<(usize, Option<i64>)> {
        if recipient.trim().is_empty() {
            return Ok((0, None));
        }
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let (predicate, params) = Self::unseen_for_recipient_predicate(recipient, factory_session);
        let sql = format!("SELECT COUNT(*), MIN(q.created_at) {predicate}");
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let (count, oldest): (i64, Option<String>) =
            conn.query_row(&sql, rusqlite::params_from_iter(param_refs), |row| {
                Ok((row.get(0)?, row.get(1)?))
            })?;
        let oldest_age = oldest
            .and_then(|created| DateTime::parse_from_rfc3339(&created).ok())
            .map(|created| {
                (Utc::now() - created.with_timezone(&Utc))
                    .num_seconds()
                    .max(0)
            });
        Ok((usize::try_from(count).unwrap_or(0), oldest_age))
    }
}

impl PromptQueueStore for SqlitePromptQueueStore {
    fn init(&self) -> Result<()> {
        // cas-88d8: concurrent openers race on check-then-ALTER. SQLite
        // auto-commits DDL, so do not wrap ADD COLUMN in ImmediateTx.
        // ensure_column + with_write_retry; indexes only after columns exist.
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            conn.execute_batch(PROMPT_QUEUE_SCHEMA)?;
            let first_lifecycle_migration =
                !crate::shared_db::column_exists(&conn, "prompt_queue", "highest_stage");

            for (col, mig) in [
                ("factory_session", PROMPT_QUEUE_SESSION_MIGRATION),
                ("summary", PROMPT_QUEUE_SUMMARY_MIGRATION),
                ("priority", PROMPT_QUEUE_PRIORITY_MIGRATION),
                ("acked_at", PROMPT_QUEUE_ACKED_AT_MIGRATION),
                ("urgent", PROMPT_QUEUE_URGENT_MIGRATION),
                ("attribution_json", PROMPT_QUEUE_ATTRIBUTION_MIGRATION),
                ("origin_agent_id", PROMPT_QUEUE_ORIGIN_MIGRATION),
                ("origin_kind", PROMPT_QUEUE_ORIGIN_KIND_MIGRATION),
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
                ("acked_via", PROMPT_QUEUE_ACKED_VIA_MIGRATION),
                ("assumed_seen_at", PROMPT_QUEUE_ASSUMED_SEEN_AT_MIGRATION),
                ("dedupe_key", PROMPT_QUEUE_DEDUPE_KEY_MIGRATION),
                ("wake_attempt", PROMPT_QUEUE_WAKE_ATTEMPT_MIGRATION),
                ("wake_attempt_at", PROMPT_QUEUE_WAKE_ATTEMPT_AT_MIGRATION),
                (
                    "wake_attempt_detail",
                    PROMPT_QUEUE_WAKE_ATTEMPT_DETAIL_MIGRATION,
                ),
                (
                    "wake_gate_declines",
                    PROMPT_QUEUE_WAKE_GATE_DECLINES_MIGRATION,
                ),
                (
                    "delivery_stalled_notified_at",
                    PROMPT_QUEUE_DELIVERY_STALLED_NOTIFIED_AT_MIGRATION,
                ),
            ] {
                crate::shared_db::ensure_column(&conn, "prompt_queue", col, mig)?;
            }

            // cas-dcf2 (GH #390): historical reply inference was stored as a
            // full acknowledgement. Preserve the activity timestamp but
            // demote it on upgrade: no historical row has transcript evidence
            // solely because a later outbound message happened.
            conn.execute(
                "UPDATE prompt_queue
                 SET assumed_seen_at = COALESCE(assumed_seen_at, acked_at),
                     acked_at = NULL,
                     acked_via = NULL,
                     highest_stage = 'assumed_seen',
                     last_pending_reason = 'awaiting_ack',
                     last_pending_detail = 'historical reply activity; awaiting message-specific confirmation'
                 WHERE acked_via = 'inferred_from_reply'",
                [],
            )?;

            // cas-7a01 (GH #155): the receipt table predates the surfacing
            // path, so existing databases need the provenance column added.
            crate::shared_db::ensure_column(
                &conn,
                "prompt_queue_recipient_seen",
                "source",
                PROMPT_QUEUE_RECIPIENT_SEEN_SOURCE_MIGRATION,
            )?;

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

    fn record_wake_gate_decline(&self, prompt_id: i64, detail: &str) -> Result<u32> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            conn.execute(
                "UPDATE prompt_queue
                 SET wake_gate_declines = COALESCE(wake_gate_declines, 0) + 1,
                     wake_attempt = 'nudge_not_attempted',
                     wake_attempt_at = ?,
                     wake_attempt_detail = ?
                 WHERE id = ? AND acked_at IS NULL AND processed_at IS NULL",
                params![Utc::now().to_rfc3339(), detail, prompt_id],
            )?;
            let declines: Option<i64> = conn
                .query_row(
                    "SELECT wake_gate_declines FROM prompt_queue WHERE id = ?",
                    params![prompt_id],
                    |row| row.get(0),
                )
                .optional()?
                .flatten();
            Ok(declines.unwrap_or(0).try_into().unwrap_or(u32::MAX))
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
        origin: Option<&QueueOrigin>,
    ) -> Result<EnqueueOutcome> {
        self.enqueue_attributed_urgent_with_outcome(
            source,
            target,
            prompt,
            factory_session,
            summary,
            priority,
            urgent,
            None,
            origin,
        )
    }

    fn enqueue_worker_peer_with_supervisor_copy(
        &self,
        source: &str,
        recipient: &str,
        supervisor: &str,
        prompt: &str,
        supervisor_copy: &str,
        factory_session: &str,
        summary: Option<&str>,
        priority: Option<NotificationPriority>,
        urgent: bool,
        origin: Option<&QueueOrigin>,
    ) -> Result<WorkerPeerMessageEnqueue> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            let now = Utc::now();
            let cutoff = (now - chrono::Duration::seconds(WORKER_PEER_MESSAGE_BURST_WINDOW_SECS))
                .to_rfc3339();
            let recent: i64 = tx.query_row(
                "SELECT COUNT(*) FROM prompt_queue \
                 WHERE source = ?1 AND target = ?2 AND factory_session = ?3 AND created_at >= ?4",
                params![source, recipient, factory_session, cutoff],
                |row| row.get(0),
            )?;
            if recent >= WORKER_PEER_MESSAGE_BURST_LIMIT {
                return Err(StoreError::Other(format!(
                    "worker peer message rate limit: at most {WORKER_PEER_MESSAGE_BURST_LIMIT} messages per minute to one peer"
                )));
            }

            let now_text = now.to_rfc3339();
            let priority: i32 = priority.unwrap_or(NotificationPriority::Normal).into();
            // cas-d9a8: both rows carry the SAME stamp, because both were
            // written by the same authenticated caller in one transaction. The
            // supervisor copy is the row that can earn a wake, so getting this
            // wrong in either direction is the bug.
            let origin_kind = origin.map(QueueOrigin::kind_str);
            let origin_agent_id = origin.and_then(QueueOrigin::agent_id);
            tx.execute(
                "INSERT INTO prompt_queue (source, target, prompt, created_at, factory_session, summary, priority, urgent, origin_agent_id, origin_kind) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?)",
                params![source, supervisor, supervisor_copy, now_text, factory_session, summary, priority, origin_agent_id, origin_kind],
            )?;
            let supervisor_copy_id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO prompt_queue (source, target, prompt, created_at, factory_session, summary, priority, urgent, origin_agent_id, origin_kind) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![source, recipient, prompt, now_text, factory_session, summary, priority, i64::from(urgent), origin_agent_id, origin_kind],
            )?;
            let recipient_id = tx.last_insert_rowid();
            let _ = capture_message_event(&tx, source, supervisor);
            let _ = capture_message_event(&tx, source, recipient);
            tx.commit()?;
            Ok(WorkerPeerMessageEnqueue {
                recipient_id,
                supervisor_copy_id,
            })
        })
    }

    fn enqueue_attributed_urgent_with_outcome(
        &self,
        source: &str,
        target: &str,
        prompt: &str,
        factory_session: Option<&str>,
        summary: Option<&str>,
        priority: Option<NotificationPriority>,
        urgent: bool,
        attribution: Option<&serde_json::Value>,
        origin: Option<&QueueOrigin>,
    ) -> Result<EnqueueOutcome> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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
                        params![source, target, prompt, factory_session, duplicate_cutoff],
                        |row| row.get::<_, i64>(0),
                    )
                    .optional()?;
                if let Some(existing_id) = delivered_duplicate {
                    tx.commit()?;
                    return Ok(EnqueueOutcome::SuppressedDuplicate(existing_id));
                }
            }

            let attribution_json = attribution.map(serde_json::to_string).transpose()?;
            // cas-d9a8: the stamp comes from the `origin` argument only. It is
            // never read back off `source` or `attribution_json` — both of
            // those are what the sender said about itself.
            let origin_kind = origin.map(QueueOrigin::kind_str);
            let origin_agent_id = origin.and_then(QueueOrigin::agent_id);
            tx.execute(
                "INSERT INTO prompt_queue (source, target, prompt, created_at, factory_session, summary, priority, urgent, attribution_json, origin_agent_id, origin_kind) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![source, target, prompt, now_text, factory_session, summary, prio, urgent_flag, attribution_json, origin_agent_id, origin_kind],
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
        origin: Option<&QueueOrigin>,
    ) -> Result<EnqueueIdempotentResult> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let now = Utc::now().to_rfc3339();
            let prio: i32 = priority.unwrap_or(NotificationPriority::Normal).into();

            // cas-d9a8: stamped on insert only. A replay that hits the
            // IGNORE keeps the FIRST writer's stamp — a later caller must not
            // be able to re-attribute a row that already exists by resending
            // its dedupe key.
            let origin_kind = origin.map(QueueOrigin::kind_str);
            let origin_agent_id = origin.and_then(QueueOrigin::agent_id);
            let changed = conn.execute(
                "INSERT OR IGNORE INTO prompt_queue
                    (source, target, prompt, created_at, factory_session, summary, priority, urgent, dedupe_key, origin_agent_id, origin_kind)
                 VALUES (?, ?, ?, ?, ?, ?, ?, 0, ?, ?, ?)",
                params![
                    source,
                    target,
                    prompt,
                    now,
                    factory_session,
                    summary,
                    prio,
                    dedupe_key,
                    origin_agent_id,
                    origin_kind
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

    fn delivery_stalled_candidates(
        &self,
        factory_session: &str,
        priority_threshold_secs: i64,
        normal_threshold_secs: i64,
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>> {
        let now = Utc::now();
        let cutoff = |label: &str, threshold_secs: i64| -> Result<String> {
            let threshold_secs = threshold_secs.max(0);
            let duration = chrono::Duration::try_seconds(threshold_secs).ok_or_else(|| {
                StoreError::Parse(format!(
                    "delivery-stalled {label} threshold {threshold_secs}s exceeds chrono's supported duration"
                ))
            })?;
            now.checked_sub_signed(duration)
                .map(|value| value.to_rfc3339())
                .ok_or_else(|| {
                    StoreError::Parse(format!(
                        "delivery-stalled {label} threshold {threshold_secs}s exceeds the supported timestamp range"
                    ))
                })
        };
        // Validate both thresholds before taking the queue mutex. An invalid
        // operator value must return a normal store error, never panic while
        // holding the lock and poison every later coordination operation.
        let priority_cutoff = cutoff("priority", priority_threshold_secs)?;
        let normal_cutoff = cutoff("normal", normal_threshold_secs)?;
        let stale_cutoff = cutoff("stale TTL", PROMPT_QUEUE_STALE_TTL_SECS)?;
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare(
            "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session, origin_agent_id, origin_kind
             FROM prompt_queue q
             WHERE q.target <> 'all_workers'
               AND q.source <> 'all_workers'
               AND q.source NOT LIKE 'lifecycle:%'
               AND q.source NOT LIKE 'lifecycle-wake:%'
               AND q.factory_session = ?
               AND q.created_at >= ?
               AND (q.dedupe_key IS NULL OR q.dedupe_key NOT LIKE 'delivery-stalled:%')
               AND EXISTS (
                   SELECT 1 FROM agents sender
                    WHERE sender.name = q.source
                      AND sender.factory_session = q.factory_session
               )
               AND q.delivery_stalled_notified_at IS NULL
               AND q.acked_at IS NULL
               AND COALESCE(q.highest_stage, 'enqueued') NOT IN ('confirmed', 'dropped', 'suppressed', 'abandoned')
               AND NOT EXISTS (
                    SELECT 1 FROM prompt_queue_recipient_seen seen
                     WHERE seen.prompt_id = q.id AND seen.recipient = q.target
               )
               AND (((q.urgent = 1 OR q.priority <= 1) AND q.created_at <= ?)
                    OR (q.urgent = 0 AND q.priority > 1 AND q.created_at <= ?))
             ORDER BY q.priority ASC, q.id ASC
             LIMIT ?",
        )?;
        Ok(stmt
            .query_map(
                params![
                    factory_session,
                    stale_cutoff,
                    priority_cutoff,
                    normal_cutoff,
                    limit as i64
                ],
                Self::prompt_from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn enqueue_delivery_stalled_bounce(
        &self,
        prompt_id: i64,
        factory_session: &str,
        notice: &str,
        summary: &str,
    ) -> Result<Option<i64>> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            let stale_cutoff =
                (Utc::now() - chrono::Duration::seconds(PROMPT_QUEUE_STALE_TTL_SECS)).to_rfc3339();
            let original = tx
                .query_row(
                    "SELECT source, factory_session FROM prompt_queue q
                     WHERE q.id = ?
                       AND q.target <> 'all_workers'
                       AND q.source <> 'all_workers'
                       AND q.source NOT LIKE 'lifecycle:%'
                       AND q.source NOT LIKE 'lifecycle-wake:%'
                       AND q.factory_session = ?
                       AND q.created_at >= ?
                       AND (q.dedupe_key IS NULL OR q.dedupe_key NOT LIKE 'delivery-stalled:%')
                       AND EXISTS (
                           SELECT 1 FROM agents sender
                            WHERE sender.name = q.source
                              AND sender.factory_session = q.factory_session
                       )
                       AND q.delivery_stalled_notified_at IS NULL
                       AND q.acked_at IS NULL
                       AND COALESCE(q.highest_stage, 'enqueued') NOT IN ('confirmed', 'dropped', 'suppressed', 'abandoned')
                       AND NOT EXISTS (
                           SELECT 1 FROM prompt_queue_recipient_seen seen
                            WHERE seen.prompt_id = q.id AND seen.recipient = q.target
                       )",
                    params![prompt_id, factory_session, stale_cutoff],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
                )
                .optional()?;
            let Some((sender, factory_session)) = original else {
                return Ok(None);
            };

            let now = Utc::now().to_rfc3339();
            let dedupe_key = format!("{DELIVERY_STALLED_BOUNCE_DEDUPE_PREFIX}{prompt_id}");
            tx.execute(
                "INSERT OR IGNORE INTO prompt_queue
                    (source, target, prompt, created_at, factory_session, summary, priority, urgent, dedupe_key)
                 VALUES ('delivery-watchdog', ?, ?, ?, ?, ?, ?, 0, ?)",
                params![
                    sender,
                    notice,
                    now,
                    factory_session,
                    summary,
                    i32::from(NotificationPriority::High),
                    dedupe_key
                ],
            )?;
            let bounce_id: i64 = tx.query_row(
                "SELECT id FROM prompt_queue WHERE dedupe_key = ?",
                params![dedupe_key],
                |row| row.get(0),
            )?;
            tx.execute(
                "UPDATE prompt_queue SET delivery_stalled_notified_at = ? WHERE id = ?",
                params![now, prompt_id],
            )?;
            tx.commit()?;
            Ok(Some(bounce_id))
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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let now = Utc::now().to_rfc3339();

            let (sql, prompt_params): (&str, Vec<Box<dyn rusqlite::ToSql>>) = if let Some(session) =
                factory_session
            {
                (
                    "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session, origin_agent_id, origin_kind
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
                    "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session, origin_agent_id, origin_kind
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

    fn poll_unseen_for_recipient(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>> {
        self.drain_unseen_for_recipient(
            recipient,
            factory_session,
            limit,
            SurfacingSource::InboxPoll,
            None,
        )
    }

    /// cas-7a01 (GH #155): the turn-start counterpart of the inbox drain.
    /// Identical eligibility and identical atomicity; the receipt it writes
    /// carries different provenance and the row is acked as hook-surfaced.
    fn surface_unseen_for_recipient(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>> {
        self.drain_unseen_for_recipient(
            recipient,
            factory_session,
            limit,
            SurfacingSource::HookSurfaced,
            None,
        )
    }

    fn surface_unseen_from_sources_for_recipient(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
        sources: &[&str],
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>> {
        self.drain_unseen_for_recipient(
            recipient,
            factory_session,
            limit,
            SurfacingSource::HookSurfaced,
            Some(sources),
        )
    }

    fn record_recipient_surfaced(
        &self,
        prompt_id: i64,
        recipient: &str,
        source: SurfacingSource,
    ) -> Result<()> {
        if recipient.trim().is_empty() {
            return Ok(());
        }
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            conn.execute(
                "INSERT OR IGNORE INTO prompt_queue_recipient_seen
                     (prompt_id, recipient, seen_at, source)
                 VALUES (?, ?, ?, ?)",
                params![
                    prompt_id,
                    recipient,
                    Utc::now().to_rfc3339(),
                    source.as_str()
                ],
            )?;
            Ok(())
        })
    }

    fn record_wake_attempt(
        &self,
        prompt_id: i64,
        attempt: WakeAttempt,
        detail: Option<&str>,
    ) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let now = Utc::now().to_rfc3339();
            // Never downgrade a recorded `Fired` to `NotAttempted`: a later
            // pass that declines to nudge says nothing about the wake this row
            // already received, and erasing it would recreate exactly the
            // blind spot this column exists to remove.
            let sql = if attempt == WakeAttempt::NotAttempted {
                "UPDATE prompt_queue
                    SET wake_attempt = ?, wake_attempt_at = ?, wake_attempt_detail = ?
                  WHERE id = ?
                    AND (wake_attempt IS NULL OR wake_attempt = 'nudge_not_attempted')"
            } else {
                "UPDATE prompt_queue
                    SET wake_attempt = ?, wake_attempt_at = ?, wake_attempt_detail = ?
                  WHERE id = ?"
            };
            conn.execute(sql, params![attempt.as_str(), now, detail, prompt_id])?;
            Ok(())
        })
    }

    fn count_unseen_for_recipient(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
    ) -> Result<usize> {
        Ok(self
            .unseen_for_recipient_summary(recipient, factory_session)?
            .0)
    }

    fn oldest_unseen_age_secs_for_recipient(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
    ) -> Result<Option<i64>> {
        Ok(self
            .unseen_for_recipient_summary(recipient, factory_session)?
            .1)
    }

    fn peek_unseen_for_recipient(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
        limit: usize,
    ) -> Result<Vec<QueuedPrompt>> {
        if recipient.trim().is_empty() || limit == 0 {
            return Ok(Vec::new());
        }
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let (predicate, mut params) =
            Self::unseen_for_recipient_predicate(recipient, factory_session);
        let sql = format!(
            "SELECT q.id, q.source, q.target, q.prompt, q.created_at, q.processed_at, q.summary, q.priority, q.acked_at, q.urgent, q.factory_session, q.origin_agent_id, q.origin_kind
             {predicate}
             ORDER BY q.id ASC
             LIMIT ?"
        );
        params.push(Box::new(limit as i64));
        let param_refs: Vec<&dyn rusqlite::ToSql> = params.iter().map(|p| p.as_ref()).collect();
        let mut stmt = conn.prepare_cached(&sql)?;
        let prompts = stmt
            .query_map(
                rusqlite::params_from_iter(param_refs),
                Self::prompt_from_row,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(prompts)
    }

    fn poll_all(&self, limit: usize) -> Result<Vec<QueuedPrompt>> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let now = Utc::now().to_rfc3339();

            let mut stmt = conn.prepare_cached(
            "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session, origin_agent_id, origin_kind
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
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let mut stmt = conn.prepare_cached(
            "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session, origin_agent_id, origin_kind
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

        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let now = Utc::now().to_rfc3339();
        // cas-d047 (GH #69): a row this old was never consumed by anyone;
        // delivering it now would hand a live worker an instruction from a
        // session that ended long ago. Withheld here even before the sweep
        // that formally quarantines it has run.
        let stale_cutoff =
            (Utc::now() - chrono::Duration::seconds(PROMPT_QUEUE_STALE_TTL_SECS)).to_rfc3339();

        // Legacy path (no session): single-lane target filter.
        let Some(session) = factory_session else {
            let placeholders: Vec<&str> = std::iter::repeat_n("?", targets.len()).collect();
            let sql = format!(
                "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session, origin_agent_id, origin_kind
                 FROM (
                     SELECT *, ROW_NUMBER() OVER (
                         PARTITION BY target, priority ORDER BY id ASC
                     ) AS cas_target_rn
                     FROM prompt_queue
                     WHERE processed_at IS NULL
                       AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
                       AND created_at >= ?
                       {NOT_ALREADY_CONSUMED_SQL}
                       AND target IN ({})
                 )
                 ORDER BY priority ASC, cas_target_rn ASC, id ASC
                 LIMIT ?",
                placeholders.join(", ")
            );
            let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![
                Box::new(now.clone()) as Box<dyn rusqlite::ToSql>,
                Box::new(stale_cutoff.clone()) as Box<dyn rusqlite::ToSql>,
            ];
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
        let session_sql = format!("SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session, origin_agent_id, origin_kind
             FROM (
                 SELECT *, ROW_NUMBER() OVER (
                     PARTITION BY target, priority ORDER BY id ASC
                 ) AS cas_target_rn
                 FROM prompt_queue
                 WHERE processed_at IS NULL
                   AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
                   AND created_at >= ?
                   {NOT_ALREADY_CONSUMED_SQL}
                   AND factory_session = ?
                   AND target IN ({})
             )
             ORDER BY priority ASC, cas_target_rn ASC, id ASC
             LIMIT ?", placeholders.join(", "));
        let mut session_params: Vec<Box<dyn rusqlite::ToSql>> = vec![
            Box::new(now.clone()),
            Box::new(stale_cutoff.clone()),
            Box::new(session.to_string()),
        ];
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
                "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session, origin_agent_id, origin_kind
                 FROM (
                     SELECT *, ROW_NUMBER() OVER (
                         PARTITION BY target, priority ORDER BY id ASC
                     ) AS cas_target_rn
                     FROM prompt_queue
                     WHERE processed_at IS NULL
                       AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
                       AND created_at >= ?
                       {NOT_ALREADY_CONSUMED_SQL}
                       AND factory_session IS NULL
                       AND target IN ({})
                 )
                 ORDER BY priority ASC, cas_target_rn ASC, id ASC
                 LIMIT ?",
                placeholders.join(", ")
            );
            let mut legacy_params: Vec<Box<dyn rusqlite::ToSql>> = vec![
                Box::new(now) as Box<dyn rusqlite::ToSql>,
                Box::new(stale_cutoff) as Box<dyn rusqlite::ToSql>,
            ];
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

        let conn = crate::shared_db::lock_connection(&self.conn)?;
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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let now = Utc::now().to_rfc3339();
            // Atomic stage advance to Confirmed (legal from Delivered or Partial).
            let _ = Self::atomic_stage_stamp(
                &conn,
                prompt_id,
                DeliveryStage::Confirmed,
                AtomicStampOpts::clear_reason(),
            );
            conn.execute(
                "UPDATE prompt_queue SET acked_at = ?, acked_via = 'explicit_ack' \
                 WHERE id = ? AND acked_at IS NULL",
                params![now, prompt_id],
            )?;
            // rows_affected == 0 means either not found or already acked — both idempotent
            Ok(())
        }) // with_write_retry
    }

    fn ack_by_dedupe_key(&self, dedupe_key: &str) -> Result<Option<i64>> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let prompt_id = conn
                .query_row(
                    "SELECT id FROM prompt_queue WHERE dedupe_key = ? LIMIT 1",
                    params![dedupe_key],
                    |row| row.get::<_, i64>(0),
                )
                .optional()?;
            let Some(prompt_id) = prompt_id else {
                return Ok(None);
            };

            let now = Utc::now().to_rfc3339();
            let _ = Self::atomic_stage_stamp(
                &conn,
                prompt_id,
                DeliveryStage::Confirmed,
                AtomicStampOpts::clear_reason(),
            );
            conn.execute(
                "UPDATE prompt_queue SET acked_at = ?, acked_via = 'explicit_ack' \
                 WHERE id = ? AND acked_at IS NULL",
                params![now, prompt_id],
            )?;
            Ok(Some(prompt_id))
        })
    }

    fn rewrite_pending(&self, prompt_id: i64, prompt: &str, summary: Option<&str>) -> Result<bool> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let changed = conn.execute(
                "UPDATE prompt_queue
                 SET prompt = ?, summary = ?
                 WHERE id = ?
                   AND processed_at IS NULL
                   AND acked_at IS NULL
                   AND (highest_stage IS NULL OR highest_stage IN ('queued', 'selected'))",
                params![prompt, summary, prompt_id],
            )?;
            Ok(changed > 0)
        })
    }

    fn ack_delivered_for_recipient(
        &self,
        recipient_aliases: &[&str],
        sender_aliases: &[&str],
        factory_session: Option<&str>,
        reply_enqueued_at: DateTime<Utc>,
    ) -> Result<usize> {
        if recipient_aliases.is_empty() || sender_aliases.is_empty() {
            return Ok(0);
        }

        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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
            // cas-99d2 (GH #126): candidate selection first, then the two
            // evidence gates evaluated in Rust.
            //
            // Timestamps are compared after `parse_datetime`, never as SQL
            // string inequalities: the column holds a mix of `to_rfc3339()`
            // output ("…+00:00") and literal "Z" spellings, and "Z" sorts
            // after "+", so for the same instant the two spellings compare
            // in opposite directions.
            let select_sql = format!(
                "SELECT prompt_queue.id,
                        prompt_queue.transport_delivered_at,
                        (SELECT MIN(seen.seen_at)
                           FROM prompt_queue_recipient_seen seen
                          WHERE seen.prompt_id = prompt_queue.id
                            AND seen.recipient = prompt_queue.target)
                 FROM prompt_queue
                 WHERE acked_at IS NULL
                   AND transport_delivered_at IS NOT NULL
                   AND target IN ({})
                   AND source IN ({})
                   {session_clause}",
                recipient_placeholders.join(", "),
                sender_placeholders.join(", "),
            );

            let mut query_params: Vec<Box<dyn rusqlite::ToSql>> =
                Vec::with_capacity(recipient_aliases.len() + sender_aliases.len() + 1);
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

            let candidates: Vec<(i64, Option<String>, Option<String>)> = {
                let mut stmt = conn.prepare(&select_sql)?;
                stmt.query_map(
                    rusqlite::params_from_iter(query_params.iter().map(|value| value.as_ref())),
                    |row| {
                        Ok((
                            row.get::<_, i64>(0)?,
                            row.get::<_, Option<String>>(1)?,
                            row.get::<_, Option<String>>(2)?,
                        ))
                    },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
            };

            let confirmable: Vec<i64> = candidates
                .into_iter()
                .filter(|(_, delivered_at, seen_at)| {
                    reply_confirms_delivered_message(
                        delivered_at.as_deref().and_then(Self::parse_datetime),
                        seen_at.as_deref().and_then(Self::parse_datetime),
                        reply_enqueued_at,
                    )
                })
                .map(|(id, _, _)| id)
                .collect();

            if confirmable.is_empty() {
                return Ok(0);
            }

            let mut stmt = conn.prepare_cached(
                "UPDATE prompt_queue
                 SET assumed_seen_at = COALESCE(assumed_seen_at, ?),
                     highest_stage = 'assumed_seen',
                     last_pending_reason = 'awaiting_ack',
                     last_pending_detail = 'later recipient activity observed; not confirmation of this message'
                 WHERE id = ?
                   AND acked_at IS NULL
                   AND assumed_seen_at IS NULL",
            )?;
            let mut updated = 0usize;
            for id in confirmable {
                updated += stmt.execute(params![now, id])?;
            }
            Ok(updated)
        })
    }

    fn unacked(&self, timeout_secs: i64, limit: usize) -> Result<Vec<QueuedPrompt>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let cutoff = (Utc::now() - chrono::Duration::seconds(timeout_secs)).to_rfc3339();

        let mut stmt = conn.prepare_cached(
            "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session, origin_agent_id, origin_kind
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
        let conn = crate::shared_db::lock_connection(&self.conn)?;
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
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let row = conn.query_row(
            "SELECT id, prompt, source, target, created_at, processed_at, factory_session,
                    priority, acked_at, urgent, selected_at, last_pending_reason,
                    last_pending_detail, transport_delivered_at, highest_stage,
                    broadcast_attempted, broadcast_succeeded, broadcast_failed,
                    acked_via, assumed_seen_at, wake_attempt, wake_attempt_at, wake_attempt_detail,
                    wake_gate_declines
             FROM prompt_queue WHERE id = ?",
            params![prompt_id],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<String>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, u8>(7).unwrap_or(2),
                    row.get::<_, Option<String>>(8)?,
                    row.get::<_, i64>(9).map(|v| v != 0).unwrap_or(false),
                    row.get::<_, Option<String>>(10).unwrap_or(None),
                    row.get::<_, Option<String>>(11).unwrap_or(None),
                    row.get::<_, Option<String>>(12).unwrap_or(None),
                    row.get::<_, Option<String>>(13).unwrap_or(None),
                    row.get::<_, Option<String>>(14).unwrap_or(None),
                    row.get::<_, Option<i64>>(15).unwrap_or(None),
                    row.get::<_, Option<i64>>(16).unwrap_or(None),
                    row.get::<_, Option<i64>>(17).unwrap_or(None),
                    row.get::<_, Option<String>>(18).unwrap_or(None),
                    row.get::<_, Option<String>>(19).unwrap_or(None),
                    row.get::<_, Option<String>>(20).unwrap_or(None),
                    row.get::<_, Option<String>>(21).unwrap_or(None),
                    row.get::<_, Option<String>>(22).unwrap_or(None),
                    row.get::<_, i64>(23).unwrap_or(0),
                ))
            },
        );

        let (
            id,
            prompt,
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
            acked_via_s,
            assumed_seen_at_s,
            wake_attempt_s,
            wake_attempt_at_s,
            wake_attempt_detail,
            wake_gate_declines,
        ) = match row {
            Ok(v) => v,
            Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(None),
            Err(e) => return Err(e.into()),
        };

        // cas-ac7e (GH #130): the recipient-side corroboration of
        // `delivered_at`. Absent only on rows delivered before this table
        // existed, or on `all_workers` (whose per-recipient transport is the
        // broadcast counts). A direct row reporting stage=delivered with this
        // field empty is the exact contradiction #130 reported.
        let recipient_transport_at = conn
            .query_row(
                "SELECT delivered_at FROM prompt_queue_recipient_transport
                  WHERE prompt_id = ? AND recipient = ?",
                params![id, &target],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .as_deref()
            .and_then(Self::parse_datetime);

        // cas-7a01 (GH #155): the only concrete artifact CAS holds that a turn
        // actually carried this row's content — the `hook_surfaced` receipt
        // written synchronously by the recipient's own `UserPromptSubmit`
        // surfacing. This is what finally makes `wake` a measurement instead
        // of the hardcoded `Unobserved` constant it was for three incidents.
        //
        // Note the deliberate asymmetry with `wake_attempt`: an `inbox_poll`
        // receipt does NOT raise `wake`. A recipient that polled its inbox
        // demonstrably took a turn on its own; the question `wake` answers is
        // whether CAS put the content in front of a recipient that would
        // otherwise never have looked.
        let hook_surfaced_at: Option<DateTime<Utc>> = conn
            .query_row(
                "SELECT seen_at FROM prompt_queue_recipient_seen
                  WHERE prompt_id = ? AND recipient = ? AND source = 'hook_surfaced'",
                params![id, &target],
                |row| row.get::<_, Option<String>>(0),
            )
            .optional()?
            .flatten()
            .as_deref()
            .and_then(Self::parse_datetime);

        let wake_evidence = hook_surfaced_at.map(|_| {
            format!(
                "prompt_queue_recipient_seen(prompt_id={id}, recipient={target}, \
                 source=hook_surfaced): UserPromptSubmit injected this row into the \
                 recipient's turn"
            )
        });

        let enqueued_at = Self::require_datetime(&created_at_s, "created_at", id)?;
        let selected_at = Self::optional_datetime(selected_at_s.as_deref(), "selected_at", id)?;
        let delivered_at = Self::optional_datetime(
            transport_delivered_s.as_deref(),
            "transport_delivered_at",
            id,
        )?;
        let confirmed_at = Self::optional_datetime(acked_at_s.as_deref(), "acked_at", id)?;
        let assumed_seen_at = Self::optional_datetime(
            assumed_seen_at_s.as_deref(),
            "assumed_seen_at",
            id,
        )?;
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

        if matches!(stage, DeliveryStage::Delivered | DeliveryStage::AssumedSeen)
            && delivered_at.is_none()
        {
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
            DeliveryStage::AssumedSeen => (
                Some(PendingReason::AwaitingAck),
                Some("later recipient activity observed; still awaiting message-specific confirmation".into()),
            ),
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
            prompt,
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
            recipient_transport_at,
            confirmed_at,
            assumed_seen_at,
            confirmation_source: ConfirmationSource::from_column(
                acked_via_s.as_deref(),
                confirmed_at.is_some(),
            ),
            pending_reason,
            pending_detail,
            broadcast_attempted: bc_attempted.map(|n| n as u32),
            broadcast_succeeded: bc_succeeded.map(|n| n as u32),
            broadcast_failed: bc_failed.map(|n| n as u32),
            wake: if hook_surfaced_at.is_some() {
                ObservationStatus::Observed
            } else {
                ObservationStatus::Unobserved
            },
            wake_attempt: WakeAttempt::from_column(wake_attempt_s.as_deref()),
            wake_gate_declines: wake_gate_declines.try_into().unwrap_or(u32::MAX),
            wake_attempt_at: Self::optional_datetime(
                wake_attempt_at_s.as_deref(),
                "wake_attempt_at",
                id,
            )?,
            wake_attempt_detail,
            wake_observed_at: hook_surfaced_at,
            wake_evidence,
            reaction: ObservationStatus::Unobserved,
            reaction_observed_at: None,
            reaction_evidence: None,
        }))
    }

    fn record_selected(&self, prompt_id: i64) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            Self::atomic_stage_stamp_in_tx(
                &tx,
                prompt_id,
                reason.implied_stage(),
                AtomicStampOpts::reason(reason, detail),
            )?;
            // cas-94a1 (GH #169): the counter and the reason that earned it are
            // stamped in ONE transaction, so the two can never disagree — the
            // way they did for all 1,121 historical rows that carry a reason
            // with a 0 counter. Only a spent transport attempt counts; see
            // `PendingReason::counts_as_delivery_attempt`.
            if reason.counts_as_delivery_attempt() {
                tx.execute(
                    "UPDATE prompt_queue
                     SET delivery_attempts = delivery_attempts + 1,
                         first_attempt_at = COALESCE(first_attempt_at, ?)
                     WHERE id = ?",
                    params![Utc::now().to_rfc3339(), prompt_id],
                )?;
            }
            tx.commit()?;
            Ok(())
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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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

    fn mark_superseded(&self, prompt_id: i64, detail: &str) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            Self::atomic_stage_stamp(
                &conn,
                prompt_id,
                DeliveryStage::Suppressed,
                AtomicStampOpts {
                    reason: Some(PendingReason::SupersededStale),
                    detail: Some(detail),
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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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

    fn mark_undelivered_lifecycle_relay(&self, prompt_id: i64, detail: Option<&str>) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            Self::atomic_stage_stamp(
                &conn,
                prompt_id,
                DeliveryStage::Abandoned,
                AtomicStampOpts {
                    reason: Some(PendingReason::UndeliveredLifecycleRelay),
                    detail,
                    set_processed: true,
                    broadcast_attempted: None,
                    broadcast_succeeded: None,
                    broadcast_failed: None,
                },
            )
        })
    }

    fn mark_undelivered_after_wake_declines(
        &self,
        prompt_id: i64,
        detail: Option<&str>,
    ) -> Result<()> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            Self::atomic_stage_stamp(
                &conn,
                prompt_id,
                DeliveryStage::Abandoned,
                AtomicStampOpts {
                    reason: Some(PendingReason::UndeliveredAfterWakeDeclines),
                    detail,
                    set_processed: true,
                    broadcast_attempted: None,
                    broadcast_succeeded: None,
                    broadcast_failed: None,
                },
            )
        })
    }

    fn list_most_retried_pending(
        &self,
        min_attempts: u32,
        limit: usize,
    ) -> Result<Vec<RetriedPrompt>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let mut stmt = conn.prepare(
            "SELECT id, source, target, summary, delivery_attempts,
                    last_pending_reason, first_attempt_at
             FROM prompt_queue
             WHERE processed_at IS NULL
               AND delivery_attempts >= ?
             ORDER BY delivery_attempts DESC, id ASC
             LIMIT ?",
        )?;
        let rows = stmt.query_map(params![min_attempts, limit as i64], |row| {
            let reason: Option<String> = row.get(5)?;
            let first_attempt_at: Option<String> = row.get(6)?;
            Ok(RetriedPrompt {
                prompt_id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                summary: row.get(3)?,
                delivery_attempts: row.get(4)?,
                reason: reason.as_deref().and_then(PendingReason::parse),
                first_attempt_at: first_attempt_at.as_deref().and_then(Self::parse_datetime),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    fn list_undelivered_lifecycle_relays(
        &self,
        limit: usize,
    ) -> Result<Vec<UndeliveredLifecycleRelay>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        // `transport_delivered_at IS NULL` is the whole test for "never
        // arrived" — it is stamped only by `mark_transport_delivered` /
        // a fully successful broadcast. Terminal stage means the row will
        // never be retried, so the failure is final rather than in progress.
        let mut stmt = conn.prepare(
            "SELECT id, source, target, summary, highest_stage, last_pending_reason,
                    last_pending_detail, factory_session, created_at, processed_at, prompt
             FROM prompt_queue
             WHERE transport_delivered_at IS NULL
               AND acked_at IS NULL
               AND source LIKE 'lifecycle-wake:%'
               AND highest_stage IN ('suppressed', 'dropped', 'abandoned')
             ORDER BY id DESC
             LIMIT ?",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let created_at: String = row.get(8)?;
            let processed_at: Option<String> = row.get(9)?;
            let reason: Option<String> = row.get(5)?;
            Ok(UndeliveredLifecycleRelay {
                prompt_id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                summary: row.get(3)?,
                prompt: row.get(10)?,
                stage: row
                    .get::<_, Option<String>>(4)?
                    .unwrap_or_else(|| DeliveryStage::Abandoned.as_str().to_string()),
                reason: reason.as_deref().and_then(PendingReason::parse),
                detail: row.get(6)?,
                factory_session: row.get(7)?,
                created_at: DateTime::parse_from_rfc3339(&created_at)
                    .map(|ts| ts.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                processed_at: processed_at.and_then(|ts| {
                    DateTime::parse_from_rfc3339(&ts)
                        .ok()
                        .map(|ts| ts.with_timezone(&Utc))
                }),
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    fn reconcile_terminal_lifecycle_relays(&self) -> Result<usize> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = ImmediateTx::new(&conn)?;
            let now = Utc::now().to_rfc3339();
            let reconciled = tx.execute(
                "UPDATE prompt_queue
                 SET acked_at = ?, acked_via = 'terminal_task_reconciled'
                 WHERE transport_delivered_at IS NULL
                   AND acked_at IS NULL
                   AND source LIKE 'lifecycle-wake:%'
                   AND highest_stage IN ('suppressed', 'dropped', 'abandoned')
                   AND EXISTS (
                       SELECT 1 FROM tasks
                       WHERE status IN ('closed', 'cancelled')
                         AND (prompt_queue.summary = tasks.id
                              OR prompt_queue.summary LIKE '%: ' || tasks.id
                              OR prompt_queue.summary LIKE '%: ' || tasks.id || ' (%')
                   )",
                params![now],
            )?;
            tx.commit()?;
            Ok(reconciled)
        })
    }

    fn undelivered_lifecycle_relay_count(&self) -> Result<usize> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM prompt_queue
             WHERE transport_delivered_at IS NULL
               AND acked_at IS NULL
               AND source LIKE 'lifecycle-wake:%'
               AND highest_stage IN ('suppressed', 'dropped', 'abandoned')",
            [],
            |row| row.get(0),
        )?;
        Ok(count.try_into().unwrap_or(usize::MAX))
    }

    fn ack_lifecycle_wake(&self, lifecycle_wake_id: i64) -> Result<Option<i64>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        let prompt_id = conn
            .query_row(
                "SELECT id FROM prompt_queue WHERE source = ? ORDER BY id DESC LIMIT 1",
                params![format!("lifecycle-wake:{lifecycle_wake_id}")],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(prompt_id) = prompt_id {
            drop(conn);
            self.ack(prompt_id)?;
        }
        Ok(prompt_id)
    }

    fn pending_count(&self) -> Result<usize> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;

        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM prompt_queue WHERE processed_at IS NULL",
            [],
            |row| row.get(0),
        )?;

        Ok(count as usize)
    }

    fn abandon_pending_older_than(&self, older_than_secs: i64) -> Result<usize> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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

    fn expire_stale_pending(&self, older_than_secs: i64) -> Result<Vec<QueuedPrompt>> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            let cutoff = (Utc::now() - chrono::Duration::seconds(older_than_secs)).to_rfc3339();
            let now = Utc::now().to_rfc3339();

            let stale: Vec<QueuedPrompt> = {
                let mut stmt = tx.prepare_cached(
                    "SELECT id, source, target, prompt, created_at, processed_at, summary, priority, acked_at, urgent, factory_session, origin_agent_id, origin_kind
                     FROM prompt_queue
                     WHERE processed_at IS NULL AND created_at < ?
                     ORDER BY id ASC",
                )?;
                stmt.query_map(params![cutoff], Self::prompt_from_row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?
            };

            if !stale.is_empty() {
                let detail = format!(
                    "stale queue item: pending {older_than_secs}s+ with no successful handoff — \
                     quarantined instead of delivered (cas-d047)"
                );
                tx.execute(
                    "UPDATE prompt_queue
                     SET processed_at = COALESCE(processed_at, ?),
                         highest_stage = 'abandoned',
                         last_pending_reason = 'abandoned_unknown_target',
                         last_pending_detail = ?,
                         next_attempt_at = NULL
                     WHERE processed_at IS NULL AND created_at < ?",
                    params![now, detail, cutoff],
                )?;
            }

            tx.commit()?;
            Ok(stale)
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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
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
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            let rows = tx.execute("DELETE FROM prompt_queue", [])?;
            tx.execute("DELETE FROM prompt_queue_recipient_seen", [])?;
            tx.commit()?;
            Ok(rows)
        }) // with_write_retry
    }

    fn cleanup_old(&self, older_than_secs: i64) -> Result<usize> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let cutoff = (Utc::now() - chrono::Duration::seconds(older_than_secs)).to_rfc3339();
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            // CI red-run rows are durable receipts keyed by branch + SHA.  They
            // must outlive ordinary delivered-prompt retention: removing one
            // would also remove its unique dedupe key and let a later watcher
            // cycle relay the already-dispositioned failure again.
            let rows = tx.execute(
                "DELETE FROM prompt_queue
                 WHERE processed_at IS NOT NULL
                   AND processed_at < ?
                   AND (dedupe_key IS NULL OR dedupe_key NOT LIKE 'ci-red-run:%')",
                params![cutoff],
            )?;
            tx.execute(
                "DELETE FROM prompt_queue_recipient_seen
                 WHERE NOT EXISTS (
                     SELECT 1 FROM prompt_queue
                     WHERE prompt_queue.id = prompt_queue_recipient_seen.prompt_id
                 )",
                [],
            )?;
            tx.commit()?;
            Ok(rows)
        }) // with_write_retry
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

impl SqlitePromptQueueStore {
    /// Shared body of [`PromptQueueStore::poll_unseen_for_recipient`] and
    /// [`PromptQueueStore::surface_unseen_for_recipient`] (cas-7a01).
    ///
    /// The two paths differ only in the provenance they record, so they must
    /// not drift in eligibility: a row the hook would surface and a row the
    /// inbox poll would drain are by definition the same row.
    fn drain_unseen_for_recipient(
        &self,
        recipient: &str,
        factory_session: Option<&str>,
        limit: usize,
        source: SurfacingSource,
        source_filter: Option<&[&str]>,
    ) -> Result<Vec<QueuedPrompt>> {
        if recipient.trim().is_empty() {
            return Err(StoreError::Other(
                "poll_unseen_for_recipient requires a recipient".to_string(),
            ));
        }
        if limit == 0 {
            return Ok(Vec::new());
        }
        let normalized_sources = source_filter
            .map(|sources| {
                sources
                    .iter()
                    .map(|value| value.trim().to_ascii_lowercase())
                    .filter(|value| !value.is_empty())
                    .collect::<std::collections::HashSet<_>>()
            })
            .unwrap_or_default();
        if source_filter.is_some() && normalized_sources.is_empty() {
            return Ok(Vec::new());
        }
        let sql_limit = i64::try_from(limit).unwrap_or(i64::MAX);

        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = crate::shared_db::ImmediateTx::new(&conn)?;
            // cas-d047 (GH #69): never hand a recipient a months-old item, and
            // (GH #70 sibling) never hand it a row the daemon already
            // terminally quarantined as dropped/suppressed/abandoned — neither
            // is actionable content, and both read as live instructions to the
            // worker that receives them.
            let stale_cutoff =
                (Utc::now() - chrono::Duration::seconds(PROMPT_QUEUE_STALE_TTL_SECS)).to_rfc3339();
            let deliverable_sql = format!(
                "AND q.created_at >= ?
                 AND (q.highest_stage IS NULL
                      OR q.highest_stage NOT IN {TERMINAL_NON_DELIVERY_STAGES})"
            );
            let source_sql = if normalized_sources.is_empty() {
                String::new()
            } else {
                let placeholders = std::iter::repeat_n("?", normalized_sources.len())
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("AND LOWER(q.source) IN ({placeholders})")
            };

            let (sql, query_params): (String, Vec<Box<dyn rusqlite::ToSql>>) =
                if let Some(session) = factory_session {
                    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![
                        Box::new(recipient.to_string()),
                        Box::new(stale_cutoff.clone()),
                        Box::new(recipient.to_string()),
                    ];
                    params.extend(
                        normalized_sources
                            .iter()
                            .cloned()
                            .map(|value| Box::new(value) as Box<dyn rusqlite::ToSql>),
                    );
                    params.push(Box::new(session.to_string()));
                    params.push(Box::new(sql_limit));
                    (
                        format!(
                            "SELECT q.id, q.source, q.target, q.prompt, q.created_at,
                                q.processed_at, q.summary, q.priority, q.acked_at,
                                q.urgent, q.factory_session, q.origin_agent_id, q.origin_kind
                         FROM prompt_queue q
                         LEFT JOIN prompt_queue_recipient_seen seen
                           ON seen.prompt_id = q.id AND seen.recipient = ?
                         WHERE seen.prompt_id IS NULL
                           {UNSURFACED_UNLESS_EXPLICIT_ACK_SQL}
                           {deliverable_sql}
                           AND (q.target = ? OR q.target = 'all_workers')
                           {source_sql}
                           AND (q.factory_session = ? OR q.factory_session IS NULL)
                         ORDER BY q.priority ASC, q.id ASC
                         LIMIT ?"
                        ),
                        params,
                    )
                } else {
                    let mut params: Vec<Box<dyn rusqlite::ToSql>> = vec![
                        Box::new(recipient.to_string()),
                        Box::new(stale_cutoff.clone()),
                        Box::new(recipient.to_string()),
                    ];
                    params.extend(
                        normalized_sources
                            .iter()
                            .cloned()
                            .map(|value| Box::new(value) as Box<dyn rusqlite::ToSql>),
                    );
                    params.push(Box::new(sql_limit));
                    (
                        format!(
                            "SELECT q.id, q.source, q.target, q.prompt, q.created_at,
                                q.processed_at, q.summary, q.priority, q.acked_at,
                                q.urgent, q.factory_session, q.origin_agent_id, q.origin_kind
                         FROM prompt_queue q
                         LEFT JOIN prompt_queue_recipient_seen seen
                           ON seen.prompt_id = q.id AND seen.recipient = ?
                         WHERE seen.prompt_id IS NULL
                           {UNSURFACED_UNLESS_EXPLICIT_ACK_SQL}
                           {deliverable_sql}
                           AND (q.target = ? OR q.target = 'all_workers')
                           {source_sql}
                           AND q.factory_session IS NULL
                         ORDER BY q.priority ASC, q.id ASC
                         LIMIT ?"
                        ),
                        params,
                    )
                };

            let prompts: Vec<QueuedPrompt> = {
                let mut stmt = tx.prepare_cached(&sql)?;
                stmt.query_map(
                    rusqlite::params_from_iter(query_params.iter().map(|p| p.as_ref())),
                    Self::prompt_from_row,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
            };

            if !prompts.is_empty() {
                let seen_at = Utc::now().to_rfc3339();
                let mut stmt = tx.prepare_cached(
                    "INSERT OR IGNORE INTO prompt_queue_recipient_seen
                         (prompt_id, recipient, seen_at, source)
                     VALUES (?, ?, ?, ?)",
                )?;
                for prompt in &prompts {
                    stmt.execute(params![prompt.id, recipient, seen_at, source.as_str()])?;
                }
                drop(stmt);

                // cas-7a01 (GH #155): a hook surfacing is a per-message,
                // per-recipient record that this content entered a turn, so it
                // confirms the row. The inbox-poll path deliberately does NOT
                // ack here — that has always been the recipient's own
                // `message_ack` decision, and changing it is out of scope.
                if source == SurfacingSource::HookSurfaced {
                    let mut ack_stmt = tx.prepare_cached(
                        "UPDATE prompt_queue
                            SET acked_at = ?, acked_via = 'hook_surfaced'
                          WHERE id = ? AND acked_at IS NULL",
                    )?;
                    for prompt in &prompts {
                        // A broadcast has one row and many recipients; one
                        // recipient's turn must not mark it confirmed for the
                        // peers that have not seen it (its read state lives in
                        // the per-recipient receipt table).
                        if prompt.target == "all_workers" {
                            continue;
                        }
                        ack_stmt.execute(params![seen_at, prompt.id])?;
                    }
                    drop(ack_stmt);
                }

                // cas-d047 (GH #70): a direct row the addressed recipient just
                // pulled has been *received* — a stronger fact than transport
                // handoff. Stamp it in the same transaction so it leaves the
                // pending set; leaving it `processed_at IS NULL` is what let a
                // later daemon tick re-write it to the inbox and re-type it
                // into an idle pane. `all_workers` is excluded: its read state
                // is per-recipient, so one drain must not consume the row for
                // peers.
                for prompt in &prompts {
                    if prompt.target == "all_workers" {
                        continue;
                    }
                    if let Err(error) = Self::atomic_stage_stamp_in_tx(
                        &tx,
                        prompt.id,
                        DeliveryStage::Delivered,
                        AtomicStampOpts::reason(
                            PendingReason::AwaitingAck,
                            Some(DRAIN_DELIVERED_DETAIL),
                        ),
                    ) {
                        // A row in a terminal non-delivery stage cannot advance
                        // to Delivered. Those are filtered out above, so this is
                        // defensive only: never fail the recipient's drain over
                        // bookkeeping.
                        tracing::debug!(
                            prompt_id = prompt.id,
                            %error,
                            "cas-d047: could not stamp drained prompt as delivered"
                        );
                        continue;
                    }

                    // cas-aac2: the hook path acked this row a few lines up, so
                    // stopping at Delivered/awaiting_ack left the raw row saying
                    // it was waiting for an ack it already holds, and naming the
                    // inbox poll as the source of a hook surfacing. Raise it to
                    // Confirmed with an accurate detail. The Delivered stamp
                    // above still runs first, so transport_delivered_at,
                    // processed_at and the per-recipient transport receipt
                    // (cas-ac7e) are written exactly as before. The inbox-poll
                    // path is untouched: it does not ack, so awaiting_ack is a
                    // true statement about it.
                    if source == SurfacingSource::HookSurfaced
                        && let Err(error) = Self::atomic_stage_stamp_in_tx(
                            &tx,
                            prompt.id,
                            DeliveryStage::Confirmed,
                            AtomicStampOpts::detail(HOOK_SURFACED_CONFIRMED_DETAIL),
                        )
                    {
                        tracing::debug!(
                            prompt_id = prompt.id,
                            %error,
                            "cas-aac2: could not stamp hook-surfaced prompt as confirmed"
                        );
                    }
                }
            }

            tx.commit()?;
            Ok(prompts)
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::prompt_queue_store::*;
    use crate::task_store::SqliteTaskStore;
    use crate::TaskStore;
    use cas_types::{Task, TaskStatus};
    use rusqlite::params;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, SqlitePromptQueueStore) {
        let temp = TempDir::new().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        (temp, store)
    }

    /// cas-d9a8: the stamped-origin columns exist and default to unattributed.
    #[test]
    fn origin_columns_are_added_and_default_to_null_for_every_existing_path() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("worker-1", "supervisor", "merge request").unwrap();
        let conn = store.conn.lock().unwrap();
        let (agent, kind): (Option<String>, Option<String>) = conn
            .query_row(
                "SELECT origin_agent_id, origin_kind FROM prompt_queue WHERE id = ?",
                params![id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        // Every pre-existing enqueue path must leave the stamp empty. A column
        // that quietly defaulted to something attributable would hand every
        // legacy and unauthenticated row a wake it never earned.
        assert_eq!(agent, None, "no enqueue path may invent an origin agent");
        assert_eq!(kind, None, "no enqueue path may invent an origin kind");
    }

    /// The classification the wake gate will depend on, pinned before the gate
    /// is re-keyed onto it (cas-d9a8).
    #[test]
    fn only_established_origins_are_attributed() {
        assert!(
            QueueOrigin::RegisteredAgent { agent_id: "agent-7".into() }.is_attributed(),
            "an authenticated registered agent is the case this exists to allow"
        );
        assert!(QueueOrigin::Daemon.is_attributed());
        assert!(
            !QueueOrigin::Unattributed.is_attributed(),
            "`cas factory message` and bridge POST cannot attribute a caller, so they must not wake"
        );
        assert_eq!(
            QueueOrigin::RegisteredAgent { agent_id: "agent-7".into() }.agent_id(),
            Some("agent-7"),
            "role resolution must key off the registry id, never a display name"
        );
        assert_eq!(QueueOrigin::Daemon.agent_id(), None);
        assert_eq!(QueueOrigin::Unattributed.kind_str(), "unattributed");
    }

    /// A stamp survives the round trip through SQLite and comes back on the
    /// row the delivery path actually reads (cas-d9a8). Without this the wake
    /// gate would be keyed on a value that is always `None` in production.
    #[test]
    fn a_stamped_row_returns_its_origin_to_the_delivery_path() {
        let (_temp, store) = create_test_store();
        let origin = QueueOrigin::RegisteredAgent {
            agent_id: "cc-4242-abc".into(),
        };
        store
            .enqueue_urgent_with_outcome(
                "worker-1",
                "supervisor",
                "merge request",
                None,
                None,
                None,
                false,
                Some(&origin),
            )
            .unwrap();
        let rows = store.peek_all(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0].origin.as_ref(),
            Some(&origin),
            "the row the daemon reads must carry the stamp the enqueue wrote"
        );

        let daemon_id = store
            .enqueue_idempotent(
                "lifecycle-wake:9",
                "supervisor",
                "<task-lifecycle …>",
                None,
                None,
                None,
                "key-9",
                Some(&QueueOrigin::Daemon),
            )
            .unwrap();
        assert!(matches!(daemon_id, EnqueueIdempotentResult::Created(_)));
        let rows = store.peek_all(10).unwrap();
        let lifecycle = rows
            .iter()
            .find(|r| r.source == "lifecycle-wake:9")
            .expect("lifecycle row");
        assert_eq!(lifecycle.origin, Some(QueueOrigin::Daemon));
    }

    /// The routes that cannot identify their caller — bridge `POST /message`
    /// and `cas factory message` — reach the store through [`enqueue`] and
    /// [`enqueue_with_session`], neither of which accepts an origin at all
    /// (cas-d9a8). This pins the consequence: no request field can put a stamp
    /// on such a row, so it can never buy a PTY write.
    #[test]
    fn unattributable_routes_cannot_stamp_an_origin() {
        let (_temp, store) = create_test_store();
        // The exact calls made by `cas factory message --from <anything>` and
        // by the bridge's `msg.from` default.
        store.enqueue("supervisor", "supervisor", "forged").unwrap();
        store
            .enqueue_with_session("lifecycle-wake:1", "supervisor", "<task-lifecycle x>", "s1")
            .unwrap();
        let rows = store.peek_all(10).unwrap();
        assert_eq!(rows.len(), 2);
        for row in rows {
            assert_eq!(
                row.origin, None,
                "a caller-settable `source` ({}) must not produce a stamp",
                row.source
            );
        }
    }

    /// A dedupe-key replay must not let a second caller re-attribute a row
    /// that already exists (cas-d9a8).
    #[test]
    fn an_idempotent_replay_keeps_the_first_writers_stamp() {
        let (_temp, store) = create_test_store();
        store
            .enqueue_idempotent(
                "lifecycle-wake:1",
                "supervisor",
                "body",
                None,
                None,
                None,
                "dupe",
                Some(&QueueOrigin::Daemon),
            )
            .unwrap();
        let replay = store
            .enqueue_idempotent(
                "lifecycle-wake:1",
                "supervisor",
                "body",
                None,
                None,
                None,
                "dupe",
                Some(&QueueOrigin::RegisteredAgent {
                    agent_id: "attacker".into(),
                }),
            )
            .unwrap();
        assert!(matches!(replay, EnqueueIdempotentResult::AlreadyExists(_)));
        let rows = store.peek_all(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].origin, Some(QueueOrigin::Daemon));
    }

    /// Corrupt provenance reads as no provenance, never as "close enough"
    /// (cas-d9a8).
    #[test]
    fn a_registered_agent_kind_without_an_id_is_not_an_origin() {
        assert_eq!(QueueOrigin::from_columns(None, Some("registered_agent")), None);
        assert_eq!(QueueOrigin::from_columns(Some("a".into()), None), None);
        assert_eq!(QueueOrigin::from_columns(None, Some("from_the_future")), None);
        assert_eq!(
            QueueOrigin::from_columns(Some("a".into()), Some("daemon")),
            Some(QueueOrigin::Daemon),
            "a stray id must not stop a daemon row being recognised"
        );
    }

    fn register_bounce_sender(store: &SqlitePromptQueueStore, name: &str, session: &str) {
        let conn = store.conn.lock().unwrap();
        conn.execute_batch(crate::AGENT_SCHEMA).unwrap();
        let now = Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO agents (id, name, factory_session, registered_at, last_heartbeat)
             VALUES (?, ?, ?, ?, ?)",
            params![format!("bounce-{name}-{session}"), name, session, now, now],
        )
        .unwrap();
    }

    #[test]
    fn attributed_enqueue_persists_metadata_on_the_delivery_row() {
        let (_temp, store) = create_test_store();
        let attribution = serde_json::json!({
            "device_id": "device-123",
            "credential_id": "credential-456",
            "device_label": "Pippenz phone",
            "operator_label": "Pippenz",
            "controller_origin": "https://commander.example",
            "request_id": "request-789"
        });
        let id = store
            .enqueue_attributed_urgent_with_outcome(
                "commander:Pippenz@Pippenz phone",
                "worker-1",
                "Please checkpoint now",
                Some("factory-1"),
                Some("checkpoint request"),
                None,
                false,
                Some(&attribution),
                None,
            )
            .unwrap()
            .id();

        let conn = store.conn.lock().unwrap();
        let (source, target, prompt, session, urgent, stored): (
            String,
            String,
            String,
            String,
            i64,
            String,
        ) = conn
            .query_row(
                "SELECT source, target, prompt, factory_session, urgent, attribution_json FROM prompt_queue WHERE id = ?",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?)),
            )
            .unwrap();
        assert_eq!(source, "commander:Pippenz@Pippenz phone");
        assert_eq!(target, "worker-1");
        assert_eq!(prompt, "Please checkpoint now");
        assert_eq!(session, "factory-1");
        assert_eq!(urgent, 0);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&stored).unwrap(),
            attribution
        );
    }

    /// cas-7787 (GH #160): a lifecycle wake relay that dies without transport
    /// must be reportable, and must read as a FAILURE rather than as the
    /// benign idle-dedup it was indistinguishable from.
    ///
    /// Reproduces the reported queue shape directly: one relay delivered
    /// (cas-dffe at 18:35, which the supervisor did receive), one wake relay
    /// terminated undelivered (cas-fe23 at 18:51, which it did not), and one
    /// ordinary idle suppression that must stay out of the report.
    #[test]
    fn an_undelivered_lifecycle_wake_relay_is_reportable_and_a_delivered_one_is_not() {
        let (_temp, store) = create_test_store();

        let delivered = store
            .enqueue_full(
                "lifecycle-wake:3375",
                "supervisor",
                "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-dffe\">",
                Some("sess"),
                Some("task_awaiting_merge: cas-dffe"),
                Some(NotificationPriority::High),
            )
            .unwrap();
        store.mark_transport_delivered(delivered).unwrap();

        let lost = store
            .enqueue_full(
                "lifecycle-wake:3386",
                "supervisor",
                "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-fe23\">",
                Some("sess"),
                Some("task_awaiting_merge: cas-fe23"),
                Some(NotificationPriority::High),
            )
            .unwrap();
        store
            .mark_undelivered_lifecycle_relay(lost, Some("task closed before delivery"))
            .unwrap();

        // Ordinary chatter suppression — not a lifecycle wake, not a failure.
        let chatter = store
            .enqueue_full(
                "worker-a",
                "supervisor",
                "standing by",
                Some("sess"),
                None,
                None,
            )
            .unwrap();
        store
            .mark_suppressed(chatter, Some("duplicate idle"))
            .unwrap();

        let reported = store.list_undelivered_lifecycle_relays(10).unwrap();
        assert_eq!(
            reported.len(),
            1,
            "exactly the relay that never arrived should be reported, got {reported:?}"
        );
        assert_eq!(reported[0].prompt_id, lost);
        assert_eq!(
            reported[0].reason,
            Some(PendingReason::UndeliveredLifecycleRelay),
            "the reason must distinguish a lost relay from `suppressed_idle` — conflating \
             the two is what made the GH #160 incident invisible"
        );
        assert_eq!(reported[0].stage, DeliveryStage::Abandoned.as_str());
    }

    /// The historical incident must be visible retroactively: rows written
    /// BEFORE this fix shipped were stamped `suppressed_idle` with a NULL
    /// `transport_delivered_at`, and those are still lost relays.
    #[test]
    fn a_legacy_suppressed_idle_wake_relay_is_still_reported_as_undelivered() {
        let (_temp, store) = create_test_store();
        let legacy = store
            .enqueue_full(
                "lifecycle-wake:3402",
                "supervisor",
                "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-edee\">",
                Some("cas-src-fast-pelican-83"),
                Some("task_awaiting_merge: cas-edee"),
                Some(NotificationPriority::High),
            )
            .unwrap();
        // Exactly what the daemon wrote on 2026-08-07 at 19:36:18.
        store
            .mark_suppressed(
                legacy,
                Some("task lifecycle occurrence no longer matches current task state"),
            )
            .unwrap();

        let reported = store.list_undelivered_lifecycle_relays(10).unwrap();
        assert_eq!(reported.len(), 1);
        assert_eq!(reported[0].prompt_id, legacy);
        assert!(
            reported[0].processed_at.is_some(),
            "the row is terminal, not in flight"
        );
    }

    /// A terminal relay remains forensic evidence in the queue, but once a
    /// supervisor explicitly acknowledges it, repeating its full banner on
    /// every `worker_status` call is pure context noise.  This pins the
    /// historical shape where a legacy suppressed row has `acked_at` set while
    /// its terminal stage remains `suppressed`.
    #[test]
    fn an_acknowledged_stale_lifecycle_relay_is_not_replayed() {
        let (_temp, store) = create_test_store();
        let stale = store
            .enqueue_full(
                "lifecycle-wake:3403",
                "supervisor",
                "<task-lifecycle transition=\"task_awaiting_merge\" task_id=\"cas-acked\">",
                Some("cas-src-fast-pelican-83"),
                Some("task_awaiting_merge: cas-acked"),
                Some(NotificationPriority::High),
            )
            .unwrap();
        store
            .mark_suppressed(
                stale,
                Some("task lifecycle occurrence no longer matches current task state"),
            )
            .unwrap();

        // Reproduce an acknowledgement written by an older receipt path: it
        // supplies durable acknowledgement evidence without rewriting the
        // historical terminal stage.
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE prompt_queue SET acked_at = ?, acked_via = 'explicit_ack' WHERE id = ?",
                params![Utc::now().to_rfc3339(), stale],
            )
            .unwrap();
        }

        assert!(
            store
                .list_undelivered_lifecycle_relays(10)
                .unwrap()
                .is_empty(),
            "an acknowledged stale relay must stay forensic in SQLite without replaying into worker_status"
        );
    }

    #[test]
    fn lifecycle_wake_id_acknowledges_the_distinct_prompt_row() {
        let (_temp, store) = create_test_store();
        let prompt_id = store
            .enqueue(
                "lifecycle-wake:4401",
                "supervisor",
                "historical terminal relay",
            )
            .unwrap();
        assert_eq!(store.ack_lifecycle_wake(4401).unwrap(), Some(prompt_id));
        assert!(
            store
                .message_delivery_report(prompt_id)
                .unwrap()
                .unwrap()
                .confirmed_at
                .is_some()
        );
    }

    #[test]
    fn terminal_task_reconciliation_drains_a_backlog_larger_than_the_display_cap() {
        let (temp, store) = create_test_store();
        let task_id = "cas-terminal-backlog";
        let task_store = SqliteTaskStore::open(temp.path()).unwrap();
        task_store.init().unwrap();
        let mut task = Task::new(task_id.to_string(), "terminal backlog".to_string());
        task_store.add(&task).unwrap();
        task.status = TaskStatus::Closed;
        task.closed_at = Some(Utc::now());
        task_store.update(&task).unwrap();
        for index in 0..12 {
            let prompt_id = store
                .enqueue_full(
                    &format!("lifecycle-wake:{}", 5100 + index),
                    "supervisor",
                    "<task-lifecycle transition=\"task_awaiting_merge\">",
                    Some("session"),
                    Some(&format!("task_awaiting_merge: {task_id} ({index})")),
                    Some(NotificationPriority::High),
                )
                .unwrap();
            store
                .mark_suppressed(prompt_id, Some("task lifecycle occurrence expired"))
                .unwrap();
        }

        assert_eq!(store.undelivered_lifecycle_relay_count().unwrap(), 12);
        assert_eq!(store.list_undelivered_lifecycle_relays(10).unwrap().len(), 10);
        assert_eq!(
            store
                .reconcile_terminal_lifecycle_relays()
                .unwrap(),
            12,
            "terminal reconciliation must drain the full queue, not only the displayed sample"
        );
        assert_eq!(store.undelivered_lifecycle_relay_count().unwrap(), 0);
        assert!(store.list_undelivered_lifecycle_relays(10).unwrap().is_empty());
        assert_eq!(
            store
                .reconcile_terminal_lifecycle_relays()
                .unwrap(),
            0,
            "a reconciled relay must never reappear on a later status read"
        );
    }

    /// cas-0147 (GH #167): a withdrawal is not idle-noise suppression.
    ///
    /// The two shared `suppressed_idle` for four days, which is how a total
    /// outage of supervisor lifecycle relays hid inside a bucket everyone
    /// reads as "benign dedup, working as intended". They must be separable by
    /// the stored reason alone, without parsing the detail string.
    #[test]
    fn a_withdrawn_payload_is_not_filed_as_idle_chatter() {
        let (_temp, store) = create_test_store();

        let chatter = store
            .enqueue("worker-a", "supervisor", "standing by")
            .unwrap();
        store
            .mark_suppressed(chatter, Some("duplicate idle"))
            .unwrap();

        let withdrawn = store
            .enqueue("lifecycle:3509", "supervisor", "<task-lifecycle ...>")
            .unwrap();
        store
            .mark_superseded(
                withdrawn,
                "withdrawn before transport: cas-0147 left the status this notification announces",
            )
            .unwrap();

        let idle = store.message_delivery_report(chatter).unwrap().unwrap();
        let sup = store.message_delivery_report(withdrawn).unwrap().unwrap();

        assert_eq!(idle.pending_reason, Some(PendingReason::SuppressedIdle));
        assert_eq!(sup.pending_reason, Some(PendingReason::SupersededStale));
        assert_ne!(
            idle.pending_reason, sup.pending_reason,
            "conflating these is the defect, not a naming preference"
        );

        // Both are terminal-without-transport, and both must remain so: this
        // change is about honesty, not about resurrecting a dead payload.
        assert_eq!(sup.stage, DeliveryStage::Suppressed);
        assert!(sup.delivered_at.is_none());
        assert_eq!(
            sup.legacy_status,
            MessageStatus::Delivered,
            "processed_at must be set — the row is terminal, not in flight"
        );
    }

    /// AC2: a terminal non-delivered row must be answerable for itself. The
    /// dead-letter carries a reason AND a detail naming what expired — a
    /// withdrawal with no recorded cause is the state this bug class lives in.
    #[test]
    fn the_dead_letter_records_why_the_row_died() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue("lifecycle:1", "supervisor", "<task-lifecycle ...>")
            .unwrap();
        store
            .mark_superseded(
                id,
                "withdrawn before transport: cas-9d92 left awaiting_merge",
            )
            .unwrap();

        let detail = store
            .message_delivery_report(id)
            .unwrap()
            .unwrap()
            .pending_detail
            .expect(
                "a dead-letter with no reason recorded is indistinguishable from a silent drop",
            );
        assert!(
            detail.contains("cas-9d92"),
            "must name what expired: {detail}"
        );
    }

    /// A withdrawal is a policy decision, not a transport that refused the
    /// handoff — it must not burn the row's retry budget (cas-d732 / cas-94a1).
    #[test]
    fn withdrawing_a_payload_does_not_spend_a_delivery_attempt() {
        assert!(!PendingReason::SupersededStale.counts_as_delivery_attempt());
        assert_eq!(
            PendingReason::parse("superseded_stale"),
            Some(PendingReason::SupersededStale),
            "the reason must survive the round trip through the stored column"
        );
    }

    /// cas-ac7e (GH #130): stamp the legacy/inferred ack shape directly.
    ///
    /// Notification 7212 was acked `inferred_from_reply` by a daemon that
    /// predates the cas-99d2 surfacing-receipt gate, so that shape cannot be
    /// produced through `ack_delivered_for_recipient` on this branch any more —
    /// but it is durably present in every store written before the upgrade,
    /// which is exactly why the drain predicate has to handle it.
    fn stamp_ack_via(store: &SqlitePromptQueueStore, prompt_id: i64, via: Option<&str>) {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE prompt_queue
                SET acked_at = ?, acked_via = ?, highest_stage = 'confirmed'
              WHERE id = ?",
            params![Utc::now().to_rfc3339(), via, prompt_id],
        )
        .unwrap();
    }

    fn recipient_transport_stamp(
        store: &SqlitePromptQueueStore,
        prompt_id: i64,
        recipient: &str,
    ) -> Option<String> {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT delivered_at FROM prompt_queue_recipient_transport
              WHERE prompt_id = ? AND recipient = ?",
            params![prompt_id, recipient],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .unwrap()
    }

    /// cas-ac7e (GH #130) AC1 — the 7183 shape.
    ///
    /// Notification 7183 read `stage=delivered` in `message_status` while the
    /// recipient's own side of the store held no delivery record for it at
    /// all: the stamp lived only on the writer's column. A Delivered stamp now
    /// leaves a per-recipient transport row in the same transaction, so the
    /// two truths cannot disagree.
    #[test]
    fn delivered_direct_row_always_carries_a_recipient_transport_stamp() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session(
                "supervisor",
                "fast-cobra-90",
                "Assignment context for cas-99d2 (pre-assigned to you)",
                "cas-src-fair-sparrow-50",
            )
            .unwrap();

        assert!(
            recipient_transport_stamp(&store, id, "fast-cobra-90").is_none(),
            "an enqueued row has not been handed to any transport yet"
        );

        store.mark_transport_delivered(id).unwrap();

        let report = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(report.stage, DeliveryStage::Delivered);
        assert!(report.delivered_at.is_some());
        assert!(
            report.recipient_transport_at.is_some(),
            "stage=delivered must imply a per-recipient transport stamp; without it \
             message_status asserts a delivery the recipient side cannot corroborate \
             — the exact 7183 contradiction"
        );
        assert_eq!(
            recipient_transport_stamp(&store, id, "fast-cobra-90"),
            report.recipient_transport_at.map(|at| at.to_rfc3339()),
            "the reported stamp must be the stored one"
        );

        // And the row the recipient actually drains is the same row, still
        // carrying that stamp — "delivered" and "drainable" are not in tension.
        let drained = store
            .poll_unseen_for_recipient("fast-cobra-90", Some("cas-src-fair-sparrow-50"), 10)
            .unwrap();
        assert_eq!(drained.iter().map(|p| p.id).collect::<Vec<_>>(), vec![id]);
        assert!(recipient_transport_stamp(&store, id, "fast-cobra-90").is_some());
    }

    /// cas-ac7e (GH #130) AC1 — re-stamping preserves the first handoff.
    #[test]
    fn recipient_transport_stamp_is_idempotent_and_keeps_the_first_instant() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("supervisor", "worker-1", "hello").unwrap();
        store.mark_transport_delivered(id).unwrap();
        let first = recipient_transport_stamp(&store, id, "worker-1").unwrap();
        store.mark_transport_delivered(id).unwrap();
        assert_eq!(
            recipient_transport_stamp(&store, id, "worker-1").unwrap(),
            first,
            "re-stamping must not rewrite the original handoff instant, matching \
             the COALESCE on transport_delivered_at"
        );
    }

    /// cas-ac7e (GH #130) AC1 — a broadcast has no single addressed recipient.
    #[test]
    fn broadcast_rows_do_not_get_a_recipient_transport_stamp() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue("supervisor", "all_workers", "stand down")
            .unwrap();
        store.mark_transport_delivered(id).unwrap();
        assert!(
            recipient_transport_stamp(&store, id, "all_workers").is_none(),
            "broadcast transport is the per-recipient broadcast counts, not a stamp \
             against the literal target 'all_workers'"
        );
    }

    /// cas-ac7e (GH #130) AC1 — the third path that reaches Delivered.
    ///
    /// `mark_broadcast_outcome` with all recipients succeeding advances the row
    /// to Delivered through the same shared stamp, so it must observe the same
    /// `all_workers` exemption. Asserted separately because the existing
    /// broadcast test only checks stage and counts and would not notice this
    /// path growing per-recipient rows keyed on the literal string.
    #[test]
    fn an_all_succeeded_broadcast_still_records_no_recipient_transport_stamp() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue("supervisor", "all_workers", "stand down")
            .unwrap();
        store.mark_broadcast_outcome(id, 3, 3, 0, None).unwrap();

        let report = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(report.stage, DeliveryStage::Delivered);
        assert!(
            report.recipient_transport_at.is_none(),
            "a broadcast has no single addressed recipient to stamp"
        );
        assert!(recipient_transport_stamp(&store, id, "all_workers").is_none());
    }

    /// cas-ac7e (GH #130) AC2 — the 7212 vanish shape.
    ///
    /// 7212 was transport-delivered, never surfaced to anyone, then stamped
    /// `acked_via = inferred_from_reply`. The drain predicate read plain
    /// `acked_at IS NULL`, so the supervisor's own full `inbox_poll` ten
    /// minutes later did not return it: an ack that was never the recipient's
    /// claim about this message had erased it from the only view that would
    /// have revealed it.
    #[test]
    fn inferred_ack_cannot_hide_a_message_the_recipient_never_saw() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session(
                "rapid-cardinal-70",
                "lively-jaguar-3",
                "Fresh after draining unread inbox until \"No unread\"",
                "cas-src-fair-sparrow-50",
            )
            .unwrap();
        store.mark_transport_delivered(id).unwrap();
        stamp_ack_via(&store, id, Some("inferred_from_reply"));

        let drained = store
            .poll_unseen_for_recipient("lively-jaguar-3", Some("cas-src-fair-sparrow-50"), 20)
            .unwrap();
        assert_eq!(
            drained.iter().map(|p| p.id).collect::<Vec<_>>(),
            vec![id],
            "a reply-inferred ack is evidence the recipient took a turn, not that \
             THIS message was put in front of it; the row must stay in the inbox"
        );
        assert_eq!(
            store
                .count_unseen_for_recipient("lively-jaguar-3", Some("cas-src-fair-sparrow-50"))
                .unwrap(),
            0,
            "the drain above wrote the surfacing receipt, so the unread count is \
             now genuinely zero"
        );

        // Second drain returns nothing: the row left the inbox by being SEEN,
        // which is the only exit that means the recipient actually got it.
        assert!(
            store
                .poll_unseen_for_recipient("lively-jaguar-3", Some("cas-src-fair-sparrow-50"), 20)
                .unwrap()
                .is_empty(),
            "a surfaced row must not re-appear forever"
        );
    }

    /// cas-ac7e (GH #130) AC2 — legacy rows with no ack provenance are treated
    /// the same as inferred ones: unknown provenance is not the recipient's
    /// claim.
    #[test]
    fn ack_without_provenance_cannot_hide_an_unsurfaced_message() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("worker-1", "supervisor", "report").unwrap();
        store.mark_transport_delivered(id).unwrap();
        stamp_ack_via(&store, id, None);

        assert_eq!(
            store
                .poll_unseen_for_recipient("supervisor", None, 20)
                .unwrap()
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            vec![id]
        );
    }

    /// cas-ac7e (GH #130) AC2 — the recipient's OWN `message_ack` is still a
    /// terminal exit. Without this the fix would resurrect every acked message.
    #[test]
    fn explicit_recipient_ack_still_clears_the_inbox() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("worker-1", "supervisor", "report").unwrap();
        store.mark_transport_delivered(id).unwrap();
        store.ack(id).unwrap();

        assert!(
            store
                .poll_unseen_for_recipient("supervisor", None, 20)
                .unwrap()
                .is_empty(),
            "explicit_ack is the recipient's claim about this message and must \
             remove it from the inbox"
        );
        assert_eq!(
            store
                .count_unseen_for_recipient("supervisor", None)
                .unwrap(),
            0
        );
    }

    /// cas-b8ce (GH #176) — THE REPRODUCTION.
    ///
    /// The live shape, from `cas.db` rows 8210/8215/8217 (zealous-fox-95) and
    /// 8221/8223/8225/8229/8241 (nimble-gazelle-41): the daemon delivered each
    /// row over the agent-teams inbox / PTY transport and stamped
    /// `transport_delivered_at`. The recipient read them, acted on them and
    /// replied. Fourteen minutes later its own `inbox_poll` handed the whole
    /// burst straight back, every receipt stamped `source='inbox_poll'` at one
    /// instant — because "unread" is `seen.prompt_id IS NULL` and the transport
    /// that did the delivering wrote no receipt.
    ///
    /// Delivery over ANY transport must be terminal for the unread view.
    #[test]
    fn a_transport_delivered_row_is_not_re_served_by_the_recipients_own_poll() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue("supervisor", "zealous-fox-95", "Assignment: cas-5c50")
            .unwrap();

        // Exactly what the daemon did for 8210: hand it to the teams-inbox
        // transport and record the handoff. No ack — the recipient had not
        // called message_ack, which is the common case in production.
        store
            .record_recipient_surfaced(id, "zealous-fox-95", SurfacingSource::TransportDelivered)
            .unwrap();
        store.mark_transport_delivered(id).unwrap();

        assert!(
            store
                .poll_unseen_for_recipient("zealous-fox-95", None, 20)
                .unwrap()
                .is_empty(),
            "a row this recipient was already shown over the daemon's own \
             transport must not come back from its inbox_poll — that is the \
             GH #176 redelivery burst"
        );
        assert_eq!(
            store
                .count_unseen_for_recipient("zealous-fox-95", None)
                .unwrap(),
            0,
            "the unread COUNT must agree with the drain, or worker_status \
             reports phantom mail"
        );
    }

    /// cas-b8ce — the receipt must not become a way to lose mail.
    ///
    /// cas-ac7e (GH #130) exists because a weak signal was allowed to erase a
    /// message from the only view that would reveal it. A receipt written for
    /// recipient A must therefore say nothing about recipient B, and must not
    /// touch the row's ack state at all.
    #[test]
    fn a_transport_receipt_is_scoped_to_one_recipient_and_is_not_an_ack() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue("supervisor", "all_workers", "standup")
            .unwrap();
        store
            .record_recipient_surfaced(id, "worker-a", SurfacingSource::TransportDelivered)
            .unwrap();

        assert!(
            store
                .poll_unseen_for_recipient("worker-a", None, 20)
                .unwrap()
                .is_empty(),
            "the receipted worker is done with this broadcast"
        );
        assert_eq!(
            store
                .poll_unseen_for_recipient("worker-b", None, 20)
                .unwrap()
                .iter()
                .map(|p| p.id)
                .collect::<Vec<_>>(),
            vec![id],
            "one worker's receipt must never hide a broadcast from a peer the \
             daemon has not delivered to yet"
        );
        assert!(
            store
                .message_delivery_report(id)
                .unwrap()
                .unwrap()
                .confirmed_at
                .is_none(),
            "delivery is not acknowledgment: a transport receipt must not \
             fabricate an ack the recipient never gave"
        );
    }

    /// cas-b8ce — idempotence. The daemon can observe the same delivery twice
    /// (a re-poll of a row whose consume raced), and `reply_confirms_delivered_message`
    /// compares the receipt instant against the reply instant — so a second
    /// observation must NOT move the receipt forward, or an already-valid
    /// reply-inference would be retroactively invalidated.
    #[test]
    fn a_repeated_transport_receipt_does_not_move_the_seen_instant() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("supervisor", "worker-1", "go").unwrap();
        store
            .record_recipient_surfaced(id, "worker-1", SurfacingSource::TransportDelivered)
            .unwrap();
        let first = recipient_seen_at(&store, id, "worker-1").expect("receipt must exist");

        store
            .record_recipient_surfaced(id, "worker-1", SurfacingSource::TransportDelivered)
            .unwrap();
        assert_eq!(
            recipient_seen_at(&store, id, "worker-1"),
            Some(first),
            "INSERT OR IGNORE: a re-observed delivery must leave the original \
             receipt instant untouched"
        );
    }

    /// Read back the persisted receipt instant for one (message, recipient).
    fn recipient_seen_at(
        store: &SqlitePromptQueueStore,
        prompt_id: i64,
        recipient: &str,
    ) -> Option<String> {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT seen_at FROM prompt_queue_recipient_seen
             WHERE prompt_id = ? AND recipient = ?",
            params![prompt_id, recipient],
            |row| row.get(0),
        )
        .ok()
    }

    /// Write the surfacing receipt an inbox drain would leave, at a chosen
    /// instant. Done in SQL rather than via `poll_unseen_for_recipient` so a
    /// test can place the receipt at a specific time and without the drain's
    /// side effect of advancing every polled row's stage (cas-99d2).
    fn record_seen(
        store: &SqlitePromptQueueStore,
        prompt_id: i64,
        recipient: &str,
        seen_at: DateTime<Utc>,
    ) {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO prompt_queue_recipient_seen (prompt_id, recipient, seen_at)
             VALUES (?, ?, ?)",
            params![prompt_id, recipient, seen_at.to_rfc3339()],
        )
        .unwrap();
    }

    /// Stamp transport delivery at a chosen instant (the real API stamps
    /// `now`, which cannot express "delivered after the reply was composed").
    fn set_transport_delivered_at(
        store: &SqlitePromptQueueStore,
        prompt_id: i64,
        at: DateTime<Utc>,
    ) {
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE prompt_queue
             SET transport_delivered_at = ?, processed_at = ?, highest_stage = 'delivered',
                 last_pending_reason = 'awaiting_ack'
             WHERE id = ?",
            params![at.to_rfc3339(), at.to_rfc3339(), prompt_id],
        )
        .unwrap();
    }

    /// cas-99d2 (GH #126): the REAL shape of notifications 7124 / 7129 in
    /// factory session cas-src-noble-salmon-99.
    ///
    /// Both were transport-delivered to a worker, both had NO
    /// `prompt_queue_recipient_seen` row, and both were nevertheless marked
    /// `confirmed` / `inferred_from_reply` by the worker's next message to the
    /// supervisor ~12s and ~21s later. That zeroed `undelivered_after` and
    /// disarmed the supervisor's escalation gate while the worker was still
    /// operating on a stale premise. The reply ordering was fine here — the
    /// missing surfacing receipt is what made the confirmation a fabrication.
    #[test]
    fn cas99d2_reply_without_a_surfacing_receipt_does_not_confirm_gh126() {
        let (_temp, store) = create_test_store();
        let delivered_at = Utc::now() - chrono::Duration::seconds(21);
        let message = store
            .enqueue_with_session(
                "supervisor",
                "fierce-crow-25",
                "factory/fierce-crow-25 is merged into the epic branch at 8823fcc3. \
                 Re-run close with commit_receipt=83179ea9 and FRESH scoped test evidence.",
                "cas-src-noble-salmon-99",
            )
            .unwrap();
        set_transport_delivered_at(&store, message, delivered_at);
        // No record_seen(...) — the worker never drained its inbox for this row.

        let confirmed = store
            .ack_delivered_for_recipient(
                &["fierce-crow-25"],
                &["supervisor"],
                Some("cas-src-noble-salmon-99"),
                Utc::now(),
            )
            .unwrap();
        assert_eq!(
            confirmed, 0,
            "a reply must not confirm a message CAS never observed being surfaced"
        );

        let report = store.message_delivery_report(message).unwrap().unwrap();
        assert_eq!(report.stage, DeliveryStage::Delivered);
        assert_eq!(report.pending_reason, Some(PendingReason::AwaitingAck));
        assert_eq!(report.confirmation_source, ConfirmationSource::Unconfirmed);
        assert!(
            report.confirmed_at.is_none(),
            "confirmed_at must stay unset so the undelivered clock keeps counting"
        );
    }

    /// cas-dcf2 (GH #390): an inbox receipt plus later activity is valuable
    /// context, but not transcript-level evidence this row entered the later
    /// turn. Keep the distinction durable and let explicit acknowledgement
    /// advance the final step separately.
    #[test]
    fn cas_dcf2_reply_after_a_surfacing_receipt_is_assumed_seen_not_confirmed() {
        let (_temp, store) = create_test_store();
        let delivered_at = Utc::now() - chrono::Duration::seconds(900);
        let seen_at = Utc::now() - chrono::Duration::seconds(19);
        let message = store
            .enqueue_with_session(
                "supervisor",
                "watchful-koala-20",
                "You are assigned task cas-7587 (P2 bug, epic cas-b0c7).",
                "cas-src-noble-salmon-99",
            )
            .unwrap();
        set_transport_delivered_at(&store, message, delivered_at);
        record_seen(&store, message, "watchful-koala-20", seen_at);

        let confirmed = store
            .ack_delivered_for_recipient(
                &["watchful-koala-20"],
                &["supervisor"],
                Some("cas-src-noble-salmon-99"),
                Utc::now(),
            )
            .unwrap();
        assert_eq!(confirmed, 1);
        let report = store.message_delivery_report(message).unwrap().unwrap();
        assert_eq!(report.stage, DeliveryStage::AssumedSeen);
        assert_eq!(report.confirmed_at, None);
        assert!(report.assumed_seen_at.is_some());
        assert_eq!(
            report.pending_reason,
            Some(PendingReason::AwaitingAck),
            "later outbound activity must leave the row escalation-eligible"
        );
    }

    #[test]
    fn cas_dcf2_init_demotes_historical_reply_inference() {
        let (_temp, store) = create_test_store();
        let message = store.enqueue("supervisor", "worker-1", "ruling").unwrap();
        store.mark_transport_delivered(message).unwrap();
        let inferred_at = Utc::now().to_rfc3339();
        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE prompt_queue
                 SET acked_at = ?, acked_via = 'inferred_from_reply', highest_stage = 'confirmed'
                 WHERE id = ?",
                params![inferred_at, message],
            )
            .unwrap();
        }

        store.init().unwrap();
        let report = store.message_delivery_report(message).unwrap().unwrap();
        assert_eq!(report.stage, DeliveryStage::AssumedSeen);
        assert!(report.confirmed_at.is_none());
        assert!(report.assumed_seen_at.is_some());
        assert_eq!(report.confirmation_source, ConfirmationSource::Unconfirmed);
    }

    #[test]
    fn cas_dcf2_wake_starved_direct_message_is_visible_not_a_lifecycle_relay() {
        let (_temp, store) = create_test_store();
        let message = store
            .enqueue("supervisor", "busy-worker", "blocking DDL ruling")
            .unwrap();

        for expected in 1..=3 {
            assert_eq!(
                store
                    .record_wake_gate_decline(
                        message,
                        "pane has not been silent long enough",
                    )
                    .unwrap(),
                expected
            );
        }
        store
            .mark_undelivered_after_wake_declines(
                message,
                Some("wake gate declined 3 consecutive re-offers while worker stayed busy"),
            )
            .unwrap();

        let report = store.message_delivery_report(message).unwrap().unwrap();
        assert_eq!(report.stage, DeliveryStage::Abandoned);
        assert_eq!(
            report.pending_reason,
            Some(PendingReason::UndeliveredAfterWakeDeclines),
            "a direct message must be explicitly wake-starved, not mislabeled as a lifecycle relay"
        );
        assert!(
            report
                .pending_detail
                .as_deref()
                .is_some_and(|detail| detail.contains("3 consecutive"))
        );
        assert_eq!(report.wake_gate_declines, 3);
    }

    /// cas-99d2 (GH #126, AC2): a reply enqueued BEFORE the message was
    /// delivered — the two crossing in flight — can never be a response to it.
    /// No production path produces this today; the ordering guard is retained
    /// so that it cannot start producing it silently.
    #[test]
    fn cas99d2_reply_composed_before_delivery_never_confirms() {
        let (_temp, store) = create_test_store();
        let reply_enqueued_at = Utc::now() - chrono::Duration::seconds(30);
        let delivered_at = reply_enqueued_at + chrono::Duration::seconds(5);
        let message = store
            .enqueue_with_session("supervisor", "worker-1", "new premise", "sess-1")
            .unwrap();
        set_transport_delivered_at(&store, message, delivered_at);
        // Receipt exists, and even predates the delivery stamp — the ordering
        // gate must still refuse, independently of the receipt gate.
        record_seen(&store, message, "worker-1", reply_enqueued_at);

        let confirmed = store
            .ack_delivered_for_recipient(
                &["worker-1"],
                &["supervisor"],
                Some("sess-1"),
                reply_enqueued_at,
            )
            .unwrap();
        assert_eq!(
            confirmed, 0,
            "a reply that predates transport delivery cannot confirm the message"
        );
        assert_eq!(
            store
                .message_delivery_report(message)
                .unwrap()
                .unwrap()
                .confirmation_source,
            ConfirmationSource::Unconfirmed
        );
    }

    /// cas-99d2: a receipt written AFTER the reply is not evidence the reply
    /// was a response — the recipient saw the content only later.
    #[test]
    fn cas99d2_surfacing_receipt_after_the_reply_does_not_confirm() {
        let (_temp, store) = create_test_store();
        let reply_enqueued_at = Utc::now() - chrono::Duration::seconds(60);
        let message = store
            .enqueue_with_session("supervisor", "worker-1", "new premise", "sess-1")
            .unwrap();
        set_transport_delivered_at(
            &store,
            message,
            reply_enqueued_at - chrono::Duration::seconds(10),
        );
        record_seen(
            &store,
            message,
            "worker-1",
            reply_enqueued_at + chrono::Duration::seconds(10),
        );

        assert_eq!(
            store
                .ack_delivered_for_recipient(
                    &["worker-1"],
                    &["supervisor"],
                    Some("sess-1"),
                    reply_enqueued_at,
                )
                .unwrap(),
            0
        );
    }

    /// cas-99d2: the gates as a pure truth table, including the sub-second
    /// precision hazard that motivated comparing parsed instants instead of
    /// SQL string inequalities.
    #[test]
    fn cas99d2_reply_confirmation_predicate_truth_table() {
        let reply = Utc::now();
        let before = reply - chrono::Duration::seconds(1);
        let after = reply + chrono::Duration::seconds(1);

        assert!(reply_confirms_delivered_message(
            Some(before),
            Some(before),
            reply
        ));
        assert!(
            reply_confirms_delivered_message(Some(reply), Some(reply), reply),
            "simultaneity is permitted; only strict inversion is refused"
        );
        assert!(
            !reply_confirms_delivered_message(Some(after), Some(before), reply),
            "delivery after the reply"
        );
        assert!(
            !reply_confirms_delivered_message(Some(before), None, reply),
            "no surfacing receipt"
        );
        assert!(
            !reply_confirms_delivered_message(Some(before), Some(after), reply),
            "receipt after the reply"
        );
        assert!(
            !reply_confirms_delivered_message(None, Some(before), reply),
            "never transport-delivered"
        );

        // Offset spelling: the store holds a mix of `to_rfc3339()` output
        // ("+00:00") and literal "Z" timestamps, and "Z" (0x5A) sorts after
        // "+" (0x2B) — so for the SAME instant a Z-spelled stamp compares
        // greater as a string. That is why the query hands parsed instants to
        // this predicate instead of using a SQL string inequality.
        let earlier = Utc.timestamp_opt(1_800_000_000, 0).unwrap();
        let later = Utc.timestamp_opt(1_800_000_000, 500_000_000).unwrap();
        let z_spelled = earlier.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        assert!(z_spelled.ends_with('Z'));
        assert!(
            z_spelled > later.to_rfc3339(),
            "the earlier instant sorts LATER as a string once offsets differ"
        );
        assert!(
            reply_confirms_delivered_message(
                SqlitePromptQueueStore::parse_datetime(&z_spelled),
                Some(earlier),
                later
            ),
            "a delivery that really is earlier must confirm regardless of how its \
             timestamp happens to be spelled"
        );
    }

    /// cas-45c4 (GH #102): later recipient activity must never be reported
    /// the same way as the recipient's own acknowledgement. It proves the
    /// recipient took a turn; it does not prove this message's content entered
    /// that later turn, so cas-dcf2 records only `AssumedSeen` rather than a
    /// confirmation.
    #[test]
    fn confirmation_source_separates_an_explicit_ack_from_a_reply_inference() {
        let (_temp, store) = create_test_store();

        let explicit = store
            .enqueue_with_session("supervisor", "swift-fox", "read this", "sess-1")
            .unwrap();
        let inferred = store
            .enqueue_with_session("supervisor", "swift-fox", "and this", "sess-1")
            .unwrap();

        // Both reach the recipient's transport.
        store.poll_all(10).unwrap();
        store.mark_transport_delivered(explicit).unwrap();
        store.mark_transport_delivered(inferred).unwrap();

        let before = store.message_delivery_report(explicit).unwrap().unwrap();
        assert_eq!(
            before.confirmation_source,
            ConfirmationSource::Unconfirmed,
            "transport handoff is not a confirmation"
        );

        // The recipient explicitly acknowledges one of them...
        store.ack(explicit).unwrap();
        // ...and separately drains its inbox (cas-99d2: the surfacing receipt
        // reply-inference now requires) and replies, which sweeps the rest.
        store
            .poll_unseen_for_recipient("swift-fox", Some("sess-1"), 10)
            .unwrap();
        store
            .ack_delivered_for_recipient(
                &["swift-fox"],
                &["supervisor"],
                Some("sess-1"),
                Utc::now(),
            )
            .unwrap();

        let a = store.message_delivery_report(explicit).unwrap().unwrap();
        let b = store.message_delivery_report(inferred).unwrap().unwrap();

        assert_eq!(a.confirmation_source, ConfirmationSource::ExplicitAck);
        assert!(
            a.confirmation_source.is_recipient_claim(),
            "the recipient acknowledged this message itself"
        );

        // Preserve the original distinction: a reply after a surfacing receipt
        // is useful evidence, but it is still not the recipient's claim about
        // this particular message.
        assert_eq!(b.stage, DeliveryStage::AssumedSeen);
        assert_eq!(b.confirmation_source, ConfirmationSource::Unconfirmed);
        assert_eq!(b.confirmed_at, None);
        assert!(b.assumed_seen_at.is_some());
        assert!(
            !b.confirmation_source.is_recipient_claim(),
            "later activity must not be presented as the recipient's claim about THIS message"
        );
        assert!(a.confirmed_at.is_some());
    }

    /// cas-45c4: rows acked before provenance tracking existed must report
    /// `Unknown` rather than being upgraded to a claim nobody made.
    #[test]
    fn a_legacy_ack_without_provenance_is_reported_as_unknown_not_explicit() {
        assert_eq!(
            ConfirmationSource::from_column(None, true),
            ConfirmationSource::Unknown
        );
        assert_eq!(
            ConfirmationSource::from_column(None, false),
            ConfirmationSource::Unconfirmed
        );
        // An ack stamp is required before any provenance is meaningful.
        assert_eq!(
            ConfirmationSource::from_column(Some("explicit_ack"), false),
            ConfirmationSource::Unconfirmed
        );
        assert!(!ConfirmationSource::Unknown.is_recipient_claim());
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
                None,
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
    fn recipient_response_marks_only_delivered_counterparty_messages_as_assumed_seen() {
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
        // cas-99d2: reply-inference also needs a surfacing receipt for the row.
        record_seen(&store, consumed, "worker-1", Utc::now());

        let assumed_seen = store
            .ack_delivered_for_recipient(
                &["worker-1"],
                &["supervisor", "display-supervisor"],
                Some("factory-session"),
                Utc::now(),
            )
            .unwrap();
        assert_eq!(assumed_seen, 1);
        // Preserve the original filter intent: only delivered messages to the
        // responding counterparty advance. cas-dcf2 makes that advance the
        // non-confirming `AssumedSeen` step.
        assert_eq!(
            store
                .message_delivery_report(consumed)
                .unwrap()
                .unwrap()
                .stage,
            DeliveryStage::AssumedSeen
        );
        let consumed_report = store.message_delivery_report(consumed).unwrap().unwrap();
        assert_eq!(consumed_report.confirmed_at, None);
        assert_eq!(
            consumed_report.confirmation_source,
            ConfirmationSource::Unconfirmed
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

    /// cas-e728 (GH #105): the count must mean "the recipient has not read
    /// this", not "the daemon has not touched it". worker_status uses it to
    /// tell a worker that is merely between turns from one that was handed
    /// work and never woke, so a row the transport already delivered must
    /// still count until the recipient actually consumes it.
    #[test]
    fn count_unseen_survives_transport_delivery_and_stops_at_recipient_read() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue("supervisor", "worker-a", "start this")
            .unwrap();

        assert_eq!(
            store.count_unseen_for_recipient("worker-a", None).unwrap(),
            1
        );

        // The daemon hands it to the transport: `processed_at` is stamped, but
        // the worker has still not read it.
        store.mark_transport_delivered(id).unwrap();
        assert_eq!(
            store.count_unseen_for_recipient("worker-a", None).unwrap(),
            1,
            "transport delivery is not the worker reading it"
        );

        // The worker polls its inbox — now it is seen.
        store
            .poll_unseen_for_recipient("worker-a", None, 10)
            .unwrap();
        assert_eq!(
            store.count_unseen_for_recipient("worker-a", None).unwrap(),
            0
        );
    }

    /// Broadcasts are real inbox items with per-recipient read state; missing
    /// them made worker_status report "inbox empty" for every worker that had
    /// just been asked to report.
    #[test]
    fn count_unseen_includes_all_workers_broadcasts_per_recipient() {
        let (_temp, store) = create_test_store();
        store
            .enqueue("supervisor", "all_workers", "everyone report")
            .unwrap();

        assert_eq!(
            store.count_unseen_for_recipient("worker-a", None).unwrap(),
            1
        );
        assert_eq!(
            store.count_unseen_for_recipient("worker-b", None).unwrap(),
            1
        );

        store
            .poll_unseen_for_recipient("worker-a", None, 10)
            .unwrap();
        assert_eq!(
            store.count_unseen_for_recipient("worker-a", None).unwrap(),
            0,
            "one worker draining a broadcast must not hide it from peers"
        );
        assert_eq!(
            store.count_unseen_for_recipient("worker-b", None).unwrap(),
            1
        );
    }

    /// cas-f08d (GH #147): the peek must return exactly the rows the count
    /// counts — worker_status classifies those rows (work message vs fired
    /// reminder) and subtracts from the count, so any drift between the two
    /// would corrupt the arithmetic. And like the count, it must not consume.
    #[test]
    fn peek_unseen_matches_the_count_and_never_consumes() {
        let (_temp, store) = create_test_store();
        store
            .enqueue("supervisor", "worker-a", "Reminder #44: check CI")
            .unwrap();
        store
            .enqueue("supervisor", "all_workers", "everyone report")
            .unwrap();
        store
            .enqueue("supervisor", "worker-b", "not yours")
            .unwrap();

        let peeked = store
            .peek_unseen_for_recipient("worker-a", None, 10)
            .unwrap();
        assert_eq!(
            peeked.len(),
            store.count_unseen_for_recipient("worker-a", None).unwrap(),
            "peek and count must agree"
        );
        assert_eq!(peeked.len(), 2, "own row + broadcast, never worker-b's");
        assert!(peeked[0].prompt.starts_with("Reminder #44:"));
        assert_eq!(peeked[1].target, "all_workers");

        // Reading it twice returns the same rows: a supervisor inspecting an
        // inbox must never mark that inbox seen.
        assert_eq!(
            store
                .peek_unseen_for_recipient("worker-a", None, 10)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            store.count_unseen_for_recipient("worker-a", None).unwrap(),
            2
        );

        // The recipient's own poll is what consumes.
        store
            .poll_unseen_for_recipient("worker-a", None, 10)
            .unwrap();
        assert!(
            store
                .peek_unseen_for_recipient("worker-a", None, 10)
                .unwrap()
                .is_empty()
        );
    }

    /// The limit bounds the read without touching the count, so a deep backlog
    /// truncates the sample rather than the total.
    #[test]
    fn peek_unseen_respects_its_limit() {
        let (_temp, store) = create_test_store();
        for n in 0..5 {
            store
                .enqueue("supervisor", "worker-a", &format!("msg {n}"))
                .unwrap();
        }

        assert_eq!(
            store
                .peek_unseen_for_recipient("worker-a", None, 2)
                .unwrap()
                .len(),
            2
        );
        assert!(
            store
                .peek_unseen_for_recipient("worker-a", None, 0)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store.count_unseen_for_recipient("worker-a", None).unwrap(),
            5,
            "the count is unaffected by how much of it was sampled"
        );
    }

    /// The age of the oldest unread row is what separates "just delivered" from
    /// "delivered and ignored"; an empty inbox has no age.
    #[test]
    fn oldest_unseen_age_is_none_for_an_empty_inbox_and_set_otherwise() {
        let (_temp, store) = create_test_store();
        assert_eq!(
            store
                .oldest_unseen_age_secs_for_recipient("worker-a", None)
                .unwrap(),
            None
        );

        store.enqueue("supervisor", "worker-a", "hello").unwrap();
        let age = store
            .oldest_unseen_age_secs_for_recipient("worker-a", None)
            .unwrap()
            .expect("a pending row must have an age");
        assert!(
            (0..5).contains(&age),
            "fresh row age should be ~0s, got {age}"
        );
    }

    /// Counting must never mark anything seen — a supervisor reading status
    /// must not consume a worker's mail.
    #[test]
    fn counting_does_not_consume_the_inbox() {
        let (_temp, store) = create_test_store();
        store.enqueue("supervisor", "worker-a", "keep me").unwrap();

        for _ in 0..3 {
            assert_eq!(
                store.count_unseen_for_recipient("worker-a", None).unwrap(),
                1
            );
        }
        assert_eq!(
            store
                .poll_unseen_for_recipient("worker-a", None, 10)
                .unwrap()
                .len(),
            1,
            "the message must still be deliverable after being counted"
        );
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
        let targets = &[
            "supervisor",
            "all_workers",
            "director",
            "active-worker",
            "stuck-target",
        ];
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
                    (Utc::now() - chrono::Duration::seconds(PROMPT_RETRY_MAX_AGE_SECS + 1))
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

    #[test]
    fn delivery_stalled_bounce_uses_priority_threshold_once_and_cancels_on_read() {
        let (_temp, store) = create_test_store();
        register_bounce_sender(&store, "supervisor", "session");
        let urgent = store
            .enqueue_full(
                "supervisor",
                "claude-worker",
                "approve refund",
                Some("session"),
                Some("refund approval"),
                Some(NotificationPriority::Critical),
            )
            .unwrap();
        let normal = store
            .enqueue_full(
                "supervisor",
                "claude-worker",
                "status update",
                Some("session"),
                Some("weekly status"),
                Some(NotificationPriority::Normal),
            )
            .unwrap();
        let old = (Utc::now() - chrono::Duration::seconds(11 * 60)).to_rfc3339();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE prompt_queue SET created_at = ? WHERE id IN (?, ?)",
                params![old, urgent, normal],
            )
            .unwrap();

        let candidates = store
            .delivery_stalled_candidates("session", 10 * 60, 30 * 60, 10)
            .unwrap();
        assert_eq!(
            candidates.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![urgent]
        );
        let bounce = store
            .enqueue_delivery_stalled_bounce(
                urgent,
                "session",
                "delivery stalled: notification_id=urgent; recipient_harness=claude; delivery_state=enqueued",
                "delivery stalled",
            )
            .unwrap()
            .expect("eligible urgent row should bounce");
        assert!(
            store
                .delivery_stalled_candidates("session", 10 * 60, 30 * 60, 10)
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            store
                .enqueue_delivery_stalled_bounce(urgent, "session", "duplicate", "duplicate")
                .unwrap(),
            None,
            "the original row can create only one sender bounce"
        );
        let bounced = store.message_delivery_report(bounce).unwrap().unwrap();
        assert_eq!(bounced.target, "supervisor");

        let very_old = (Utc::now() - chrono::Duration::seconds(31 * 60)).to_rfc3339();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE prompt_queue SET created_at = ? WHERE id = ?",
                params![very_old, normal],
            )
            .unwrap();
        assert_eq!(
            store
                .delivery_stalled_candidates("session", 10 * 60, 30 * 60, 10)
                .unwrap()
                .len(),
            1
        );
        store
            .poll_unseen_for_recipient("claude-worker", Some("session"), 10)
            .unwrap();
        assert!(
            store
                .delivery_stalled_candidates("session", 10 * 60, 30 * 60, 10)
                .unwrap()
                .is_empty(),
            "a recipient read cancels the pending sender bounce"
        );
    }

    #[test]
    fn delivery_stalled_bounce_requires_live_same_session_direct_sender() {
        let (_temp, store) = create_test_store();
        const SESSION: &str = "factory-a";
        const OTHER_SESSION: &str = "factory-b";

        register_bounce_sender(&store, "supervisor", SESSION);
        register_bounce_sender(&store, "delivery-watchdog", SESSION);
        register_bounce_sender(&store, "lifecycle-wake:42", SESSION);
        register_bounce_sender(&store, "foreign-supervisor", OTHER_SESSION);

        let genuine = store
            .enqueue_with_session("supervisor", "worker-a", "genuine aged direct", SESSION)
            .unwrap();
        let broadcast = store
            .enqueue_with_session("supervisor", "all_workers", "broadcast", SESSION)
            .unwrap();
        let synthetic = store
            .enqueue_with_session("lifecycle-wake:42", "worker-a", "synthetic", SESSION)
            .unwrap();
        let unregistered = store
            .enqueue_with_session("unknown-sender", "worker-a", "unregistered", SESSION)
            .unwrap();
        let cross_session = store
            .enqueue_with_session(
                "foreign-supervisor",
                "worker-a",
                "foreign session",
                OTHER_SESSION,
            )
            .unwrap();
        let stale = store
            .enqueue_with_session("supervisor", "worker-a", "historical", SESSION)
            .unwrap();
        let spoofed_watchdog_source = store
            .enqueue_with_session(
                "delivery-watchdog",
                "worker-a",
                "spoofed source is not a watchdog marker",
                SESSION,
            )
            .unwrap();
        let structural_watchdog = match store
            .enqueue_idempotent(
                "delivery-watchdog",
                "supervisor",
                "real watchdog row",
                Some(SESSION),
                None,
                Some(NotificationPriority::High),
                "delivery-stalled:prior-message",
                None,
            )
            .unwrap()
        {
            EnqueueIdempotentResult::Created(id) | EnqueueIdempotentResult::AlreadyExists(id) => id,
        };

        let aged = (Utc::now() - chrono::Duration::minutes(11)).to_rfc3339();
        let historical =
            (Utc::now() - chrono::Duration::seconds(PROMPT_QUEUE_STALE_TTL_SECS + 60)).to_rfc3339();
        let conn = store.conn.lock().unwrap();
        conn.execute(
            "UPDATE prompt_queue SET created_at = ? WHERE id <> ?",
            params![aged, stale],
        )
        .unwrap();
        conn.execute(
            "UPDATE prompt_queue SET created_at = ? WHERE id = ?",
            params![historical, stale],
        )
        .unwrap();
        drop(conn);

        let candidates = store
            .delivery_stalled_candidates(SESSION, 10 * 60, 10 * 60, 20)
            .unwrap();
        let candidate_ids: std::collections::HashSet<i64> =
            candidates.iter().map(|row| row.id).collect();
        assert!(candidate_ids.contains(&genuine));
        assert!(candidate_ids.contains(&spoofed_watchdog_source));
        for excluded in [
            broadcast,
            synthetic,
            unregistered,
            cross_session,
            stale,
            structural_watchdog,
        ] {
            assert!(
                !candidate_ids.contains(&excluded),
                "ineligible row {excluded} must not become a delivery-stalled bounce candidate"
            );
        }
        assert_eq!(
            store
                .enqueue_delivery_stalled_bounce(stale, SESSION, "stale", "stalled")
                .unwrap(),
            None,
            "the atomic bounce recheck must still reject a row past the stale TTL"
        );
        assert_eq!(
            store
                .enqueue_delivery_stalled_bounce(
                    structural_watchdog,
                    SESSION,
                    "watchdog",
                    "stalled",
                )
                .unwrap(),
            None,
            "the atomic bounce recheck must identify the watchdog structurally"
        );

        let first_bounce = store
            .enqueue_delivery_stalled_bounce(genuine, SESSION, "genuine bounce", "stalled")
            .unwrap()
            .expect("the genuine same-session direct message must bounce");
        assert_eq!(
            store
                .message_delivery_report(first_bounce)
                .unwrap()
                .unwrap()
                .target,
            "supervisor"
        );
        assert_eq!(
            store
                .enqueue_delivery_stalled_bounce(genuine, SESSION, "duplicate", "stalled")
                .unwrap(),
            None,
            "a genuine row still bounces exactly once"
        );
    }

    #[test]
    fn casb123_delivery_stalled_threshold_overflow_returns_error_without_poisoning_queue() {
        let (_temp, store) = create_test_store();
        register_bounce_sender(&store, "supervisor", "session");
        store
            .delivery_stalled_candidates("session", 10_i64.pow(12), 10_i64.pow(12), 10)
            .expect("a trillion-second threshold must not panic");
        let error = store
            .delivery_stalled_candidates("session", i64::MAX, 30 * 60, 10)
            .expect_err("an unrepresentable threshold must be rejected without a panic");
        assert!(
            error
                .to_string()
                .contains("delivery-stalled priority threshold"),
            "unexpected error: {error}"
        );

        store
            .enqueue("supervisor", "worker", "queue remains usable")
            .expect("threshold validation must happen before and not poison the queue mutex");
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

    /// cas-ac7e (GH #130): the recipient-transport table has to appear on
    /// stores that predate it, or every existing factory keeps reporting
    /// `stage=delivered` with no recipient-side stamp — the bug, preserved.
    #[test]
    fn recipient_transport_table_is_created_on_a_preexisting_store() {
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
                "INSERT INTO prompt_queue (source, target, prompt, created_at) \
                 VALUES ('supervisor','worker-1','legacy', datetime('now'))",
                [],
            )
            .unwrap();
        }

        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();

        // A row delivered AFTER the upgrade gets the stamp...
        let fresh = store
            .enqueue("supervisor", "worker-1", "post-upgrade")
            .unwrap();
        store.mark_transport_delivered(fresh).unwrap();
        assert!(
            recipient_transport_stamp(&store, fresh, "worker-1").is_some(),
            "the table must exist and be written on an upgraded store"
        );

        // ...and the pre-existing row is untouched, not dropped.
        assert!(
            store
                .peek_all(10)
                .unwrap()
                .iter()
                .any(|p| p.prompt == "legacy"),
            "adding the table must not disturb existing rows"
        );
    }

    /// cas-ac7e (GH #130): the recipient's own drain is a delivery path too, so
    /// it must leave the same corroboration as the daemon's handoff. Without
    /// this a row could be reported `stage=delivered` with no stamp purely
    /// because it reached the recipient by polling rather than by the daemon.
    #[test]
    fn a_drained_row_also_records_its_recipient_transport_stamp() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "worker-1", "assignment", "sess")
            .unwrap();
        assert!(recipient_transport_stamp(&store, id, "worker-1").is_none());

        let drained = store
            .poll_unseen_for_recipient("worker-1", Some("sess"), 10)
            .unwrap();
        assert_eq!(drained.len(), 1);

        let report = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(report.stage, DeliveryStage::Delivered);
        assert!(
            report.recipient_transport_at.is_some(),
            "a drain that advances the row to Delivered must leave the stamp too"
        );
    }

    /// cas-ac7e (GH #130) AC2 — `unseen_for_recipient_summary` backs the unread
    /// count and oldest-age the supervisor escalates on. If it kept the old
    /// `acked_at IS NULL` predicate, a vanished message would read as zero
    /// unread while `poll_unseen_for_recipient` still returned it.
    #[test]
    fn unread_count_agrees_with_the_drain_about_an_inferred_acked_row() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("worker-1", "supervisor", "report").unwrap();
        store.mark_transport_delivered(id).unwrap();
        stamp_ack_via(&store, id, Some("inferred_from_reply"));

        assert_eq!(
            store
                .count_unseen_for_recipient("supervisor", None)
                .unwrap(),
            1,
            "the unread count must see exactly what the drain would return"
        );
        assert!(
            store
                .oldest_unseen_age_secs_for_recipient("supervisor", None)
                .unwrap()
                .is_some(),
            "the undelivered clock must keep running on a row nobody has seen"
        );
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
                origin: None,
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
            let stale_delivered_at = (Utc::now()
                - chrono::Duration::seconds(PROMPT_DUPLICATE_WINDOW_SECS + 1))
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
            let mut stmt = conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).unwrap();
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
                None,
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
                None,
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
                None,
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

    #[test]
    fn inbox_poll_marks_seen_per_recipient_without_consuming_transport() {
        let temp = TempDir::new().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();

        let direct = store
            .enqueue_with_session("supervisor", "worker-a", "direct", "session-a")
            .unwrap();
        store
            .enqueue_with_session("supervisor", "worker-b", "other worker", "session-a")
            .unwrap();
        let broadcast = store
            .enqueue_with_session("supervisor", "all_workers", "broadcast", "session-a")
            .unwrap();
        let legacy = store
            .enqueue("supervisor", "worker-a", "legacy direct")
            .unwrap();
        store
            .enqueue_with_session("supervisor", "worker-a", "other session", "session-b")
            .unwrap();
        let acknowledged = store
            .enqueue_with_session("supervisor", "worker-a", "already read", "session-a")
            .unwrap();
        store.ack(acknowledged).unwrap();

        let first = store
            .poll_unseen_for_recipient("worker-a", Some("session-a"), 10)
            .unwrap();
        let first_ids: Vec<i64> = first.iter().map(|prompt| prompt.id).collect();
        assert_eq!(first_ids, vec![direct, broadcast, legacy]);

        assert!(
            store
                .poll_unseen_for_recipient("worker-a", Some("session-a"), 10)
                .unwrap()
                .is_empty(),
            "the same recipient must not receive a second inbox-poll copy"
        );
        // cas-d047 (GH #70) revises this for DIRECT rows only: a message the
        // addressed recipient pulled itself has been received, so the row is
        // consumed rather than left pending for a later daemon tick to write
        // to the inbox again and re-type into an idle pane. Broadcast rows keep
        // the original per-recipient contract asserted just below.
        assert_eq!(
            store.message_status(direct).unwrap(),
            Some(MessageStatus::Delivered),
            "a direct row drained by its recipient is consumed, not left pending"
        );
        assert_eq!(
            store.message_status(broadcast).unwrap(),
            Some(MessageStatus::Pending),
            "broadcast transport must remain pending after one recipient polls"
        );

        store.ack(broadcast).unwrap();
        let peer = store
            .poll_unseen_for_recipient("worker-b", Some("session-a"), 10)
            .unwrap();
        assert!(
            peer.iter().any(|prompt| prompt.id == broadcast),
            "worker A polling and acknowledging a broadcast must not hide it from worker B"
        );
    }

    #[test]
    fn inbox_poll_cleanup_removes_matching_and_preexisting_orphan_seen_rows() {
        let (_temp, store) = create_test_store();
        let old = store.enqueue("supervisor", "worker-a", "old").unwrap();
        store
            .poll_unseen_for_recipient("worker-a", None, 10)
            .unwrap();

        {
            let conn = store.conn.lock().unwrap();
            conn.execute(
                "UPDATE prompt_queue
                 SET processed_at = '2000-01-01T00:00:00Z'
                 WHERE id = ?",
                [old],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO prompt_queue_recipient_seen (prompt_id, recipient, seen_at)
                 VALUES (999999, 'orphan-worker', '2000-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        }

        assert_eq!(store.cleanup_old(0).unwrap(), 1);
        let seen_count: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT COUNT(*) FROM prompt_queue_recipient_seen",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(
            seen_count, 0,
            "cleanup_old must remove both deleted-row state and prior orphans"
        );
    }

    #[test]
    fn inbox_poll_clear_removes_all_seen_rows_including_orphans() {
        let (_temp, store) = create_test_store();
        store.enqueue("supervisor", "worker-a", "message").unwrap();
        store
            .poll_unseen_for_recipient("worker-a", None, 10)
            .unwrap();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO prompt_queue_recipient_seen (prompt_id, recipient, seen_at)
                 VALUES (999999, 'orphan-worker', '2000-01-01T00:00:00Z')",
                [],
            )
            .unwrap();

        assert_eq!(store.clear().unwrap(), 1);
        let conn = store.conn.lock().unwrap();
        let prompt_count: i64 = conn
            .query_row("SELECT COUNT(*) FROM prompt_queue", [], |row| row.get(0))
            .unwrap();
        let seen_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM prompt_queue_recipient_seen",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!((prompt_count, seen_count), (0, 0));
    }

    #[test]
    fn concurrent_same_recipient_polls_on_two_connections_do_not_duplicate_delivery() {
        use std::sync::{Arc, Barrier, Mutex};
        use std::time::Duration;

        fn independent_store(path: &Path) -> SqlitePromptQueueStore {
            let conn = Connection::open(path.join("cas.db")).unwrap();
            conn.busy_timeout(Duration::from_secs(5)).unwrap();
            conn.execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;
                 PRAGMA foreign_keys=ON;",
            )
            .unwrap();
            SqlitePromptQueueStore {
                conn: Arc::new(Mutex::new(conn)),
            }
        }

        let temp = TempDir::new().unwrap();
        let first_store = independent_store(temp.path());
        first_store.init().unwrap();
        first_store
            .enqueue("supervisor", "worker-a", "only once")
            .unwrap();
        let second_store = independent_store(temp.path());
        second_store.init().unwrap();
        let barrier = Arc::new(Barrier::new(2));

        let (first, second) = std::thread::scope(|scope| {
            let first_barrier = Arc::clone(&barrier);
            let first = scope.spawn(move || {
                first_barrier.wait();
                first_store
                    .poll_unseen_for_recipient("worker-a", None, 10)
                    .unwrap()
            });
            let second_barrier = Arc::clone(&barrier);
            let second = scope.spawn(move || {
                second_barrier.wait();
                second_store
                    .poll_unseen_for_recipient("worker-a", None, 10)
                    .unwrap()
            });
            (first.join().unwrap(), second.join().unwrap())
        });

        assert_eq!(
            first.len() + second.len(),
            1,
            "the IMMEDIATE claim transaction must deliver a row to only one connection"
        );
    }

    // ---------------------------------------------------------------------
    // cas-d047 — message-queue hygiene (GH #70 redelivery, GH #69 stale items)
    // ---------------------------------------------------------------------

    /// Backdate a row's `created_at` so age-based rules can be exercised
    /// without sleeping.
    fn backdate(store: &SqlitePromptQueueStore, id: i64, age_secs: i64) {
        let created = (Utc::now() - chrono::Duration::seconds(age_secs)).to_rfc3339();
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE prompt_queue SET created_at = ? WHERE id = ?",
                params![created, id],
            )
            .unwrap();
    }

    /// GH #70 core: once the addressed recipient has drained a message through
    /// its own inbox poll, the daemon must never select that row again — the
    /// idle-nudge path re-delivered exactly these rows because a recipient
    /// drain left `processed_at` NULL.
    #[test]
    fn drained_message_is_never_reselected_by_the_daemon() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "worker-a", "start cas-1234", "sess-1")
            .unwrap();

        let drained = store
            .poll_unseen_for_recipient("worker-a", Some("sess-1"), 10)
            .unwrap();
        assert_eq!(drained.len(), 1);
        assert_eq!(drained[0].id, id);

        let peeked = store
            .peek_for_targets(&["worker-a"], Some("sess-1"), 10)
            .unwrap();
        assert!(
            peeked.is_empty(),
            "a message already drained by its recipient must not be re-delivered, got {peeked:?}"
        );
        assert_eq!(
            store.pending_count().unwrap(),
            0,
            "a drained direct row must leave the pending set instead of lingering forever"
        );
        let report = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(report.stage, DeliveryStage::Delivered);
    }

    /// A second poll by the same recipient is already suppressed by the seen
    /// table; this pins that drain remains idempotent after the row is stamped.
    #[test]
    fn draining_twice_returns_nothing_the_second_time() {
        let (_temp, store) = create_test_store();
        store
            .enqueue_with_session("supervisor", "worker-a", "contract addendum", "sess-1")
            .unwrap();

        assert_eq!(
            store
                .poll_unseen_for_recipient("worker-a", Some("sess-1"), 10)
                .unwrap()
                .len(),
            1
        );
        assert!(
            store
                .poll_unseen_for_recipient("worker-a", Some("sess-1"), 10)
                .unwrap()
                .is_empty()
        );
    }

    /// GH #70, second shape: an acknowledged message (the recipient answered
    /// the sender) must also drop out of daemon selection.
    #[test]
    fn acked_message_is_never_reselected_by_the_daemon() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "worker-a", "please merge", "sess-1")
            .unwrap();
        store.mark_transport_delivered(id).unwrap();
        // Re-open the row for selection the way a retry would (processed_at is
        // the daemon's own bookkeeping; the ack is the recipient's).
        store
            .conn
            .lock()
            .unwrap()
            .execute(
                "UPDATE prompt_queue SET processed_at = NULL WHERE id = ?",
                params![id],
            )
            .unwrap();
        store.ack(id).unwrap();

        assert!(
            store
                .peek_for_targets(&["worker-a"], Some("sess-1"), 10)
                .unwrap()
                .is_empty(),
            "an acked message must not be re-delivered"
        );
    }

    /// The recipient-scoped exclusion must not collapse broadcasts: one
    /// worker draining an `all_workers` row cannot hide it from the daemon
    /// (which still has to deliver it to every other worker).
    #[test]
    fn broadcast_row_survives_one_recipient_drain() {
        let (_temp, store) = create_test_store();
        store
            .enqueue_with_session("supervisor", "all_workers", "standup", "sess-1")
            .unwrap();

        assert_eq!(
            store
                .poll_unseen_for_recipient("worker-a", Some("sess-1"), 10)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            store
                .peek_for_targets(&["worker-a", "worker-b", "all_workers"], Some("sess-1"), 10)
                .unwrap()
                .len(),
            1,
            "a broadcast must stay deliverable to peers after one recipient drains it"
        );
        assert_eq!(
            store
                .poll_unseen_for_recipient("worker-b", Some("sess-1"), 10)
                .unwrap()
                .len(),
            1,
            "peers must still see the broadcast"
        );
    }

    /// GH #69: a months-old undelivered queue item must be expired with a
    /// reportable record instead of waiting for any worker whose name matches.
    #[test]
    fn expire_stale_pending_quarantines_ancient_rows() {
        let (_temp, store) = create_test_store();
        let stale = store
            .enqueue("supervisor", "wise-raven-21", "verify+close cas-85c0")
            .unwrap();
        backdate(&store, stale, 130 * 24 * 3600);
        let fresh = store
            .enqueue("supervisor", "wise-raven-21", "start cas-4717")
            .unwrap();

        let expired = store
            .expire_stale_pending(PROMPT_QUEUE_STALE_TTL_SECS)
            .unwrap();
        assert_eq!(expired.len(), 1, "only the ancient row expires");
        assert_eq!(expired[0].id, stale);
        assert_eq!(expired[0].target, "wise-raven-21");

        let report = store.message_delivery_report(stale).unwrap().unwrap();
        assert_eq!(report.stage, DeliveryStage::Abandoned);
        assert_eq!(
            report.pending_reason,
            Some(PendingReason::AbandonedUnknownTarget)
        );
        assert!(
            report
                .pending_detail
                .as_deref()
                .is_some_and(|d| d.contains("stale")),
            "expiry must leave a forensic detail, got {:?}",
            report.pending_detail
        );

        // Idempotent: a second sweep has nothing left to expire.
        assert!(
            store
                .expire_stale_pending(PROMPT_QUEUE_STALE_TTL_SECS)
                .unwrap()
                .is_empty()
        );

        let peeked = store
            .peek_for_targets(&["wise-raven-21"], None, 10)
            .unwrap();
        assert_eq!(peeked.len(), 1, "the fresh row is untouched");
        assert_eq!(peeked[0].id, fresh);
    }

    /// GH #69, delivery-side guarantee: even if no sweep has run yet, a
    /// freshly spawned worker's inbox poll must not hand it a months-old item,
    /// and the daemon must not inject one either.
    #[test]
    fn stale_rows_are_not_delivered_to_a_newly_spawned_worker() {
        let (_temp, store) = create_test_store();
        let stale = store
            .enqueue("supervisor", "wise-raven-21", "verify+close cas-85c0")
            .unwrap();
        backdate(&store, stale, 130 * 24 * 3600);

        assert!(
            store
                .poll_unseen_for_recipient("wise-raven-21", None, 10)
                .unwrap()
                .is_empty(),
            "a stale cross-session item must not reach a new worker's inbox"
        );
        assert!(
            store
                .peek_for_targets(&["wise-raven-21"], None, 10)
                .unwrap()
                .is_empty(),
            "a stale item must not be injected by the daemon either"
        );
    }

    /// A row the daemon terminally quarantined (dropped/suppressed/abandoned)
    /// is not deliverable content: the recipient inbox poll must skip it.
    #[test]
    fn inbox_poll_skips_terminally_quarantined_rows() {
        let (_temp, store) = create_test_store();
        let abandoned = store.enqueue("supervisor", "worker-a", "ghost").unwrap();
        store
            .mark_abandoned(abandoned, Some("target not in session"))
            .unwrap();
        let suppressed = store
            .enqueue("worker-a", "worker-a", "standing by")
            .unwrap();
        store
            .mark_suppressed(suppressed, Some("duplicate idle"))
            .unwrap();
        let live = store
            .enqueue("supervisor", "worker-a", "real work")
            .unwrap();

        let polled = store
            .poll_unseen_for_recipient("worker-a", None, 10)
            .unwrap();
        assert_eq!(polled.len(), 1, "only the live row is deliverable");
        assert_eq!(polled[0].id, live);
    }

    // ---- cas-7a01 (GH #155): wake observability + turn-start surfacing ----

    /// AC1. `wake: unobserved` used to be a constant with no backing column,
    /// so a nudge that fired, one that failed and one that was never attempted
    /// were the same string in `message_status`. Each must now round-trip.
    #[test]
    fn wake_attempt_states_round_trip_through_the_report() {
        let (_temp, store) = create_test_store();
        for (attempt, detail) in [
            (WakeAttempt::Fired, None),
            (WakeAttempt::Failed, Some("pane inject failed: broken pipe")),
            (WakeAttempt::NotAttempted, Some("idle gate declined")),
        ] {
            let id = store.enqueue("supervisor", "worker-a", "work").unwrap();
            store.record_wake_attempt(id, attempt, detail).unwrap();
            let report = store.message_delivery_report(id).unwrap().unwrap();
            assert_eq!(report.wake_attempt, attempt);
            assert_eq!(report.wake_attempt_detail.as_deref(), detail);
            assert!(
                report.wake_attempt_at.is_some(),
                "a recorded attempt must carry when it happened"
            );
        }
    }

    /// A row with no wake bookkeeping at all — every row written before this
    /// column existed — must read as `NotAttempted`, never as a fired nudge.
    #[test]
    fn a_row_with_no_wake_record_reports_not_attempted() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("supervisor", "worker-a", "work").unwrap();
        let report = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(report.wake_attempt, WakeAttempt::NotAttempted);
        assert_eq!(report.wake_attempt_at, None);
    }

    /// A later pass that declines to nudge says nothing about a wake this row
    /// already received. Letting it overwrite `Fired` would recreate the blind
    /// spot the column exists to remove.
    #[test]
    fn a_later_non_attempt_cannot_erase_a_recorded_wake() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("supervisor", "worker-a", "work").unwrap();
        store
            .record_wake_attempt(id, WakeAttempt::Fired, None)
            .unwrap();
        store
            .record_wake_attempt(id, WakeAttempt::NotAttempted, Some("gate declined"))
            .unwrap();
        assert_eq!(
            store
                .message_delivery_report(id)
                .unwrap()
                .unwrap()
                .wake_attempt,
            WakeAttempt::Fired
        );
    }

    /// A genuine failure after a fired nudge IS newer information and must land.
    #[test]
    fn a_failure_after_a_fire_is_recorded() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("supervisor", "worker-a", "work").unwrap();
        store
            .record_wake_attempt(id, WakeAttempt::Fired, None)
            .unwrap();
        store
            .record_wake_attempt(id, WakeAttempt::Failed, Some("pane gone"))
            .unwrap();
        assert_eq!(
            store
                .message_delivery_report(id)
                .unwrap()
                .unwrap()
                .wake_attempt,
            WakeAttempt::Failed
        );
    }

    /// AC2/AC3, the reproduction. A message enqueued seconds AFTER the worker
    /// drained its inbox to "No unread messages" is the exact shape reported in
    /// GH #155: the drain consumed nothing because nothing had arrived yet, and
    /// the row then had no path to the worker. It must surface at the next turn
    /// start.
    #[test]
    fn a_message_arriving_just_after_a_drain_surfaces_at_the_next_turn() {
        let (_temp, store) = create_test_store();

        // The worker drains: empty inbox, exactly as the incident reported.
        let drained = store
            .poll_unseen_for_recipient("ready-cheetah-71", Some("session"), 10)
            .unwrap();
        assert!(drained.is_empty(), "precondition: the inbox drained empty");

        // The supervisor's message lands seconds later.
        let id = store
            .enqueue_with_session(
                "supervisor",
                "ready-cheetah-71",
                "start cas-7a01",
                "session",
            )
            .unwrap();

        // The worker takes its next turn.
        let surfaced = store
            .surface_unseen_for_recipient("ready-cheetah-71", Some("session"), 10)
            .unwrap();
        assert_eq!(
            surfaced.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![id],
            "a post-drain message must reach the worker's next turn"
        );
        assert_eq!(surfaced[0].prompt, "start cas-7a01");
    }

    /// AC1's recipient-side half: once the hook has injected a row into a turn,
    /// `message_status` reports an OBSERVED wake with concrete provenance —
    /// not the blanket `unobserved` that made three incidents unreadable.
    #[test]
    fn a_hook_surfaced_row_reports_an_observed_wake_with_evidence() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "worker-a", "work", "session")
            .unwrap();

        let before = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(before.wake, ObservationStatus::Unobserved);

        store
            .surface_unseen_for_recipient("worker-a", Some("session"), 10)
            .unwrap();

        let after = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(after.wake, ObservationStatus::Observed);
        assert!(after.wake_observed_at.is_some());
        assert!(
            after
                .wake_evidence
                .as_deref()
                .is_some_and(|e| e.contains("hook_surfaced")),
            "an observed wake must name the artifact that proves it: {:?}",
            after.wake_evidence
        );
        assert_eq!(after.confirmation_source, ConfirmationSource::HookSurfaced);
        assert!(
            after.confirmation_source.is_recipient_claim(),
            "a per-message injection record is a claim about THIS message"
        );
    }

    /// An `inbox_poll` drain must NOT raise the wake observation: a recipient
    /// that polled demonstrably took a turn on its own. Conflating the two
    /// would make every healthy poll look like a rescued message.
    #[test]
    fn an_inbox_poll_drain_does_not_claim_an_observed_wake() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "worker-a", "work", "session")
            .unwrap();
        store
            .poll_unseen_for_recipient("worker-a", Some("session"), 10)
            .unwrap();
        let report = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(report.wake, ObservationStatus::Unobserved);
        assert_ne!(report.confirmation_source, ConfirmationSource::HookSurfaced);
    }

    #[test]
    fn source_filtered_surface_leaves_unrelated_startup_mail_unread() {
        let (_temp, store) = create_test_store();
        let correction = store
            .enqueue_with_session(
                "bright-supervisor",
                "worker-a",
                "Do not touch the generated files.",
                "session",
            )
            .unwrap();
        let task_brief = store
            .enqueue_with_session(
                "director",
                "worker-a",
                "Start the pre-assigned task.",
                "session",
            )
            .unwrap();

        let surfaced = store
            .surface_unseen_from_sources_for_recipient(
                "worker-a",
                Some("session"),
                &["BRIGHT-SUPERVISOR"],
                10,
            )
            .unwrap();
        assert_eq!(
            surfaced.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![correction],
            "the transition gate must surface only the supervisor correction"
        );

        let still_unread = store
            .peek_unseen_for_recipient("worker-a", Some("session"), 10)
            .unwrap();
        assert_eq!(
            still_unread.iter().map(|row| row.id).collect::<Vec<_>>(),
            vec![task_brief],
            "the daemon-generated task brief must remain on its normal delivery path"
        );
    }

    /// The GH #124 / cas-ceae storm guard, stated as the supervisor required:
    /// a receipted row is never injected again, on any later turn.
    #[test]
    fn a_receipted_row_is_never_injected_into_a_second_turn() {
        let (_temp, store) = create_test_store();
        store
            .enqueue_with_session("supervisor", "worker-a", "do it", "session")
            .unwrap();

        let first = store
            .surface_unseen_for_recipient("worker-a", Some("session"), 10)
            .unwrap();
        assert_eq!(first.len(), 1);

        for turn in 0..3 {
            let later = store
                .surface_unseen_for_recipient("worker-a", Some("session"), 10)
                .unwrap();
            assert!(
                later.is_empty(),
                "turn {turn} re-injected an already-surfaced row (the #124 storm)"
            );
        }
    }

    /// The other half of the same invariant: selection and receipt share one
    /// transaction, so a single surfacing call can never hand back the same row
    /// twice — no duplicate content within one turn.
    #[test]
    fn one_surfacing_call_never_returns_a_row_twice() {
        let (_temp, store) = create_test_store();
        for n in 0..5 {
            store
                .enqueue_with_session("supervisor", "worker-a", &format!("msg {n}"), "session")
                .unwrap();
        }
        let surfaced = store
            .surface_unseen_for_recipient("worker-a", Some("session"), 10)
            .unwrap();
        let mut ids: Vec<i64> = surfaced.iter().map(|r| r.id).collect();
        let total = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(
            ids.len(),
            total,
            "the same row was injected twice in one turn"
        );
        assert_eq!(total, 5);
    }

    /// Surfacing and polling must agree on what is deliverable: they are the
    /// same rows read by two paths, so any eligibility drift would make a
    /// message visible to one and invisible to the other.
    #[test]
    fn surfacing_and_polling_select_the_same_rows() {
        let (_temp, store) = create_test_store();
        let abandoned = store.enqueue("supervisor", "worker-a", "stale").unwrap();
        store
            .mark_abandoned(abandoned, Some("target gone"))
            .unwrap();
        let live = store
            .enqueue("supervisor", "worker-a", "real work")
            .unwrap();

        let surfaced = store
            .surface_unseen_for_recipient("worker-a", None, 10)
            .unwrap();
        assert_eq!(
            surfaced.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![live]
        );
    }

    /// A broadcast has one row and many recipients. One agent's turn must not
    /// confirm it for peers who have never seen it.
    #[test]
    fn surfacing_a_broadcast_does_not_confirm_it_for_peers() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "all_workers", "stand down", "session")
            .unwrap();

        let first = store
            .surface_unseen_for_recipient("worker-a", Some("session"), 10)
            .unwrap();
        assert_eq!(first.len(), 1, "the first worker sees the broadcast");

        let report = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(
            report.confirmed_at, None,
            "one recipient's turn must not ack a broadcast for everyone"
        );

        let peer = store
            .surface_unseen_for_recipient("worker-b", Some("session"), 10)
            .unwrap();
        assert_eq!(peer.len(), 1, "a peer must still receive the broadcast");
    }
}

/// cas-aac2: the raw `prompt_queue` row a hook surfacing leaves behind is what
/// the delivery-mining analysis reads, so it must describe what actually
/// happened. Before this fix the hook path acked the row and then stamped
/// `delivered` / `awaiting_ack` / "consumed by recipient inbox poll" over it —
/// a row waiting for an ack it already held, crediting the wrong source.
#[cfg(test)]
mod cas_aac2_hook_surfaced_stamp_tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, SqlitePromptQueueStore) {
        let temp = TempDir::new().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        (temp, store)
    }

    struct RawRow {
        highest_stage: Option<String>,
        pending_reason: Option<String>,
        pending_detail: Option<String>,
        acked_at: Option<String>,
        acked_via: Option<String>,
        transport_delivered_at: Option<String>,
        processed_at: Option<String>,
    }

    /// Read the columns the mining scripts read, not the report's derived view.
    fn raw_row(store: &SqlitePromptQueueStore, id: i64) -> RawRow {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT highest_stage, last_pending_reason, last_pending_detail,
                    acked_at, acked_via, transport_delivered_at, processed_at
             FROM prompt_queue WHERE id = ?",
            params![id],
            |row| {
                Ok(RawRow {
                    highest_stage: row.get(0)?,
                    pending_reason: row.get(1)?,
                    pending_detail: row.get(2)?,
                    acked_at: row.get(3)?,
                    acked_via: row.get(4)?,
                    transport_delivered_at: row.get(5)?,
                    processed_at: row.get(6)?,
                })
            },
        )
        .unwrap()
    }

    /// AC1: a hook-acked row ends at `confirmed`, with a detail naming hook
    /// surfacing and no lingering `awaiting_ack`.
    #[test]
    fn a_hook_surfaced_row_ends_at_confirmed_naming_the_hook() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "worker-a", "work", "session")
            .unwrap();

        store
            .surface_unseen_for_recipient("worker-a", Some("session"), 10)
            .unwrap();

        let raw = raw_row(&store, id);
        assert_eq!(
            raw.acked_via.as_deref(),
            Some("hook_surfaced"),
            "precondition: the hook path acked the row"
        );
        assert!(raw.acked_at.is_some());
        assert_eq!(
            raw.highest_stage.as_deref(),
            Some(DeliveryStage::Confirmed.as_str()),
            "an acked row must reach Confirmed in the raw table, not stop at delivered"
        );
        assert_eq!(
            raw.pending_reason, None,
            "a confirmed row is not pending on anything; awaiting_ack was the misleading state"
        );
        let detail = raw.pending_detail.unwrap_or_default();
        assert!(
            detail.contains("hook surfacing"),
            "the detail must name hook surfacing: {detail:?}"
        );
        assert!(
            !detail.contains("inbox poll"),
            "the detail must not credit the inbox poll for a hook surfacing: {detail:?}"
        );
    }

    /// The Confirmed stamp rides ON TOP of the cas-d047 Delivered stamp, so the
    /// transport bookkeeping that keeps a drained row out of the pending set is
    /// still written — including the cas-ac7e per-recipient transport receipt.
    #[test]
    fn confirming_preserves_the_transport_bookkeeping() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "worker-a", "work", "session")
            .unwrap();
        store
            .surface_unseen_for_recipient("worker-a", Some("session"), 10)
            .unwrap();

        let raw = raw_row(&store, id);
        assert!(
            raw.transport_delivered_at.is_some(),
            "transport_delivered_at must still be stamped"
        );
        assert!(
            raw.processed_at.is_some(),
            "processed_at must still be stamped or the daemon can re-type the row"
        );

        let report = store.message_delivery_report(id).unwrap().unwrap();
        assert!(
            report.recipient_transport_at.is_some(),
            "the cas-ac7e per-recipient transport receipt must survive"
        );
    }

    /// AC2: the inbox-poll path is untouched. It does not ack, so
    /// `delivered` / `awaiting_ack` / "inbox poll" remains the true statement.
    #[test]
    fn the_inbox_poll_path_still_stamps_delivered_awaiting_ack() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "worker-a", "work", "session")
            .unwrap();

        store
            .poll_unseen_for_recipient("worker-a", Some("session"), 10)
            .unwrap();

        let raw = raw_row(&store, id);
        assert_eq!(raw.acked_at, None, "the poll path must not ack");
        assert_eq!(
            raw.highest_stage.as_deref(),
            Some(DeliveryStage::Delivered.as_str())
        );
        assert_eq!(
            raw.pending_reason.as_deref(),
            Some(PendingReason::AwaitingAck.as_str())
        );
        assert_eq!(
            raw.pending_detail.as_deref(),
            Some(DRAIN_DELIVERED_DETAIL),
            "the inbox-poll detail is unchanged"
        );
    }

    /// A broadcast row is acked per recipient, not per row, so one worker's
    /// turn must not confirm it — the Confirmed stamp inherits that exclusion.
    #[test]
    fn surfacing_a_broadcast_does_not_confirm_the_row() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "all_workers", "stand down", "session")
            .unwrap();

        store
            .surface_unseen_for_recipient("worker-a", Some("session"), 10)
            .unwrap();

        let raw = raw_row(&store, id);
        assert_eq!(raw.acked_at, None);
        assert_ne!(
            raw.highest_stage.as_deref(),
            Some(DeliveryStage::Confirmed.as_str()),
            "one recipient's turn must not confirm a broadcast for its peers"
        );
    }

    /// AC3: no delivery decision moves. The report already derived Confirmed
    /// from `acked_at` at read time, and every `highest_stage IS NOT 'confirmed'`
    /// predicate conjoins `acked_at IS NULL` — so the row was already excluded
    /// from the unacked set before this fix and still is after it.
    #[test]
    fn the_read_time_derivation_stays_authoritative() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue_with_session("supervisor", "worker-a", "work", "session")
            .unwrap();
        store
            .surface_unseen_for_recipient("worker-a", Some("session"), 10)
            .unwrap();

        let report = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(
            report.stage,
            DeliveryStage::Confirmed,
            "the report reported Confirmed before this fix and must still"
        );
        // One honest behaviour change, and the only one: the report derives
        // pending_reason from the STORED stage (before the acked_at override),
        // so a hook-acked row used to report `awaiting_ack` alongside
        // stage=confirmed. It now reports nothing pending, which is what the
        // stage always claimed. No delivery decision reads this field.
        assert_eq!(report.pending_reason, None);
        assert_eq!(report.confirmation_source, ConfirmationSource::HookSurfaced);

        let unacked: i64 = {
            let conn = store.conn.lock().unwrap();
            conn.query_row(
                "SELECT COUNT(*) FROM prompt_queue
                  WHERE acked_at IS NULL AND highest_stage IS NOT 'confirmed'",
                [],
                |row| row.get(0),
            )
            .unwrap()
        };
        assert_eq!(
            unacked, 0,
            "the acked_at conjunction already excluded this row; the stage column only \
             ever agreed with it late"
        );
    }
}

#[cfg(test)]
mod cas_94a1_delivery_attempts_tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, SqlitePromptQueueStore) {
        let temp = TempDir::new().unwrap();
        let store = SqlitePromptQueueStore::open(temp.path()).unwrap();
        store.init().unwrap();
        (temp, store)
    }

    fn attempts_of(store: &SqlitePromptQueueStore, id: i64) -> u32 {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT delivery_attempts FROM prompt_queue WHERE id = ?",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn first_attempt_at_of(store: &SqlitePromptQueueStore, id: i64) -> Option<String> {
        let conn = store.conn.lock().unwrap();
        conn.query_row(
            "SELECT first_attempt_at FROM prompt_queue WHERE id = ?",
            params![id],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// AC(1): N real transport attempts -> N. The reported defect was 0 after
    /// thousands of attempts.
    #[test]
    fn n_real_attempts_increment_the_counter_n_times() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue("supervisor", "worker-a", "do the thing")
            .unwrap();
        assert_eq!(attempts_of(&store, id), 0);

        for expected in 1..=5 {
            store
                .record_pending_reason(id, PendingReason::AdapterRetryable, Some("inject failed"))
                .unwrap();
            assert_eq!(
                attempts_of(&store, id),
                expected,
                "each spent transport attempt must increment exactly once"
            );
        }
        assert!(
            first_attempt_at_of(&store, id).is_some(),
            "the first spent attempt must stamp first_attempt_at — it was NULL on all 8,017 \
             live rows, which is how we knew the writer had never run"
        );
    }

    /// The invariant a blanket increment would have broken: cas-d732/cas-7787
    /// require that a policy withhold does not burn the row's retry budget.
    #[test]
    fn policy_withholds_do_not_burn_the_budget() {
        let (_temp, store) = create_test_store();
        let id = store
            .enqueue("supervisor", "worker-a", "held back")
            .unwrap();

        for reason in [
            PendingReason::GatedNotReady,
            PendingReason::AwaitingDelivery,
            PendingReason::SessionIneligible,
            PendingReason::NoIntendedRecipients,
            PendingReason::AwaitingAck,
        ] {
            store
                .record_pending_reason(id, reason, Some("withheld"))
                .unwrap();
        }

        assert_eq!(
            attempts_of(&store, id),
            0,
            "a cooldown/gate/ack-wait is not a spent transport attempt"
        );
        assert!(
            first_attempt_at_of(&store, id).is_none(),
            "no attempt spent means no first_attempt_at"
        );
    }

    /// Terminal stamps must not double-count the attempt that already failed.
    #[test]
    fn terminal_stamps_do_not_double_count() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("supervisor", "worker-a", "doomed").unwrap();

        store
            .record_pending_reason(id, PendingReason::AdapterRetryable, Some("failed"))
            .unwrap();
        assert_eq!(attempts_of(&store, id), 1);

        // Stage transitions are monotonic, so each terminal reason needs its
        // own row rather than two terminal stamps on one.
        store
            .record_pending_reason(id, PendingReason::SuppressedIdle, Some("terminal"))
            .unwrap();
        assert_eq!(
            attempts_of(&store, id),
            1,
            "a terminal outcome records the death, not another attempt"
        );

        let other = store
            .enqueue("supervisor", "worker-b", "also doomed")
            .unwrap();
        store
            .record_pending_reason(other, PendingReason::AdapterRetryable, Some("failed"))
            .unwrap();
        store
            .record_pending_reason(
                other,
                PendingReason::AbandonedUnknownTarget,
                Some("terminal"),
            )
            .unwrap();
        assert_eq!(attempts_of(&store, other), 1);
    }

    /// The counter and the reason that earned it are written in ONE
    /// transaction, so they can never disagree — the exact disagreement that
    /// left 1,121 live rows carrying a reason with a 0 counter.
    #[test]
    fn counter_and_reason_are_stamped_together() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("supervisor", "worker-a", "msg").unwrap();
        store
            .record_pending_reason(id, PendingReason::TargetUnavailable, Some("no pane"))
            .unwrap();

        let report = store.message_delivery_report(id).unwrap().unwrap();
        assert_eq!(
            report.pending_reason,
            Some(PendingReason::TargetUnavailable)
        );
        assert_eq!(
            attempts_of(&store, id),
            1,
            "a stamped retryable reason with a 0 counter is the bug this fixes"
        );
    }

    /// AC(1) read side: a consumer can name the worst offenders.
    #[test]
    fn most_retried_pending_reports_worst_first() {
        let (_temp, store) = create_test_store();
        let quiet = store.enqueue("supervisor", "worker-a", "fine").unwrap();
        let bad = store
            .enqueue("supervisor", "worker-b", "struggling")
            .unwrap();
        let worst = store
            .enqueue("supervisor", "worker-c", "unreachable")
            .unwrap();

        for _ in 0..3 {
            store
                .record_pending_reason(bad, PendingReason::AdapterRetryable, None)
                .unwrap();
        }
        for _ in 0..7 {
            store
                .record_pending_reason(worst, PendingReason::TargetUnavailable, None)
                .unwrap();
        }

        let rows = store.list_most_retried_pending(3, 10).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.prompt_id).collect::<Vec<_>>(),
            vec![worst, bad],
            "worst first, and a row under the threshold is not reported"
        );
        assert_eq!(rows[0].delivery_attempts, 7);
        assert_eq!(rows[0].target, "worker-c");
        assert_eq!(rows[0].reason, Some(PendingReason::TargetUnavailable));
        assert!(rows[0].first_attempt_at.is_some());
        assert!(!rows.iter().any(|r| r.prompt_id == quiet));
    }

    /// A processed row is finished business, not a live retry problem.
    #[test]
    fn processed_rows_drop_out_of_the_retry_report() {
        let (_temp, store) = create_test_store();
        let id = store.enqueue("supervisor", "worker-a", "msg").unwrap();
        for _ in 0..4 {
            store
                .record_pending_reason(id, PendingReason::AdapterRetryable, None)
                .unwrap();
        }
        assert_eq!(store.list_most_retried_pending(3, 10).unwrap().len(), 1);

        store.mark_transport_delivered(id).unwrap();
        assert!(
            store.list_most_retried_pending(3, 10).unwrap().is_empty(),
            "a delivered row is not a pending retry problem"
        );
    }

    /// Pins the classifier itself so a new PendingReason variant cannot be
    /// added without someone deciding whether it spends an attempt.
    #[test]
    fn classifier_names_exactly_the_transport_spending_reasons() {
        for reason in [
            PendingReason::AdapterRetryable,
            PendingReason::TargetUnavailable,
        ] {
            assert!(
                reason.counts_as_delivery_attempt(),
                "{reason} spends an attempt"
            );
        }
        for reason in [
            PendingReason::GatedNotReady,
            PendingReason::SessionIneligible,
            PendingReason::AwaitingDelivery,
            PendingReason::AwaitingAck,
            PendingReason::NoIntendedRecipients,
            PendingReason::DroppedDeadSource,
            PendingReason::SuppressedIdle,
            PendingReason::AbandonedUnknownTarget,
            PendingReason::UndeliveredLifecycleRelay,
            PendingReason::UndeliveredAfterWakeDeclines,
            PendingReason::PartialBroadcast,
        ] {
            assert!(
                !reason.counts_as_delivery_attempt(),
                "{reason} must not spend a transport attempt"
            );
        }
    }
}
