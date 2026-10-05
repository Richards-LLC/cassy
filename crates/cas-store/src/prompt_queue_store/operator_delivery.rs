//! Local operator-turn recording and replayable delivery claims (cas-4c78).
//!
//! Phase 2a has no remote implementation, account enrollment or encryption.
//! Snapshots are local plaintext, like prompt history. A future remote adapter
//! must verify enrollment and seal them before crossing the machine boundary.

use super::*;

pub const OPERATOR_DELIVERY_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS operator_delivery_outbox (
        event_id TEXT PRIMARY KEY,
        prompt_id INTEGER NOT NULL UNIQUE,
        factory_session TEXT,
        audience_state TEXT NOT NULL DEFAULT 'unenrolled' CHECK (audience_state = 'unenrolled'),
        payload_snapshot TEXT NOT NULL,
        created_at TEXT NOT NULL,
        attempts INTEGER NOT NULL DEFAULT 0,
        retry_at_ms INTEGER NOT NULL DEFAULT 0,
        lease_token TEXT,
        lease_until_ms INTEGER,
        relay_receipt TEXT,
        retained_at TEXT,
        CHECK ((lease_token IS NULL) = (lease_until_ms IS NULL)),
        CHECK ((relay_receipt IS NULL) = (retained_at IS NULL))
    )",
    "CREATE INDEX IF NOT EXISTS idx_operator_delivery_pending
        ON operator_delivery_outbox(retry_at_ms, lease_until_ms, created_at, event_id)
        WHERE retained_at IS NULL",
    "CREATE TRIGGER IF NOT EXISTS operator_delivery_immutable
        BEFORE UPDATE OF event_id, prompt_id, factory_session, audience_state, payload_snapshot, created_at
        ON operator_delivery_outbox
        WHEN NEW.event_id IS NOT OLD.event_id OR NEW.prompt_id IS NOT OLD.prompt_id
          OR NEW.factory_session IS NOT OLD.factory_session
          OR NEW.audience_state IS NOT OLD.audience_state
          OR NEW.payload_snapshot IS NOT OLD.payload_snapshot OR NEW.created_at IS NOT OLD.created_at
        BEGIN SELECT RAISE(ABORT, 'operator event identity and payload are immutable'); END",
];

/// Complete caller input; identity and the local unenrolled audience are owned
/// by the store. Never synthesize account membership from these display fields.
pub struct OperatorTurn<'a> {
    pub source: &'a str,
    pub target: &'a str,
    pub prompt: &'a str,
    pub factory_session: Option<&'a str>,
    pub metadata: OperatorTurnMetadata<'a>,
}

#[derive(Default)]
pub struct OperatorTurnMetadata<'a> {
    pub summary: Option<&'a str>,
    pub priority: Option<NotificationPriority>,
    pub urgent: bool,
    pub attribution: Option<&'a serde_json::Value>,
    pub origin: Option<&'a QueueOrigin>,
    pub operator: Option<&'a OperatorStamp>,
    pub recipient_device_id: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub attachments: &'a [cas_types::ArtifactRef],
    pub dedupe_key: Option<&'a str>,
    /// Explicit reply confirmation, committed with the reply and its event.
    pub acknowledge_prompt_id: Option<i64>,
}

/// Immutable local event. No account is claimed until Phase 2b verifies it.
#[derive(Clone, Serialize, Deserialize)]
pub struct OperatorDeliveryEvent {
    pub event_id: String,
    pub prompt_id: i64,
    pub factory_session: Option<String>,
    pub audience_state: String,
    pub payload_snapshot: String,
    pub created_at: String,
}

pub struct OperatorDeliveryClaim {
    pub event: OperatorDeliveryEvent,
    pub lease_token: String,
    pub lease_until: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorRelayReceipt {
    pub event_id: String,
    pub receipt_id: String,
}

/// Transport seam, currently exercised only by an in-process fake. There is
/// deliberately no daemon scheduling or plaintext network adapter in Phase 2a.
/// Implementations must idempotently retain by event ID and reject conflicting
/// content. Phase 2b adds scoped enrollment/sealing and authenticated receipts.
pub trait OperatorDeliveryTransport {
    fn retain(&self, event: &OperatorDeliveryEvent) -> Result<OperatorRelayReceipt>;
}

#[derive(Clone, Copy)]
pub struct OperatorDrainLimits {
    pub max_events: usize,
    pub max_payload_bytes: usize,
    pub lease_seconds: i64,
}

impl Default for OperatorDrainLimits {
    fn default() -> Self {
        Self {
            max_events: 100,
            max_payload_bytes: 1024 * 1024,
            lease_seconds: 30,
        }
    }
}

impl OperatorDrainLimits {
    fn validate(self) -> Result<()> {
        if !(1..=100).contains(&self.max_events)
            || !(1..=1024 * 1024).contains(&self.max_payload_bytes)
            || !(1..=120).contains(&self.lease_seconds)
        {
            return Err(StoreError::Other(
                "operator drain limits exceed the bounded batch/lease policy".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub struct OperatorDrainReport {
    pub claimed: usize,
    pub retained: usize,
    pub retry_scheduled: usize,
    pub fenced: usize,
}

fn random_identity() -> String {
    rand::random::<[u8; 16]>()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

pub(super) fn new_event_identity() -> String {
    random_identity()
}

impl SqlitePromptQueueStore {
    /// All prompt metadata, explicit completion and immutable local outbox
    /// snapshot share this transaction. Caller-facing wrappers decide whether
    /// a row is eligible for the operator lane; ordinary queue traffic is not.
    pub(super) fn record_complete_operator_turn(
        &self,
        turn: &OperatorTurn<'_>,
    ) -> Result<EnqueueOutcome> {
        require_operator_session(turn.target, turn.factory_session)?;
        let metadata = &turn.metadata;
        if metadata
            .recipient_device_id
            .is_some_and(|id| id.trim().is_empty())
            || metadata.kind.is_some_and(|kind| kind.trim().is_empty())
            || metadata.dedupe_key.is_some_and(|key| key.trim().is_empty())
        {
            return Err(StoreError::Other(
                "operator turn metadata cannot contain empty identifiers".into(),
            ));
        }
        let event_id = random_identity();
        let outcome = crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = ImmediateTx::new(&conn)?;
            let outcome = Self::insert_complete_operator_turn(&tx, turn, &event_id)?;
            tx.commit()?;
            Ok(outcome)
        })?;
        if matches!(outcome, EnqueueOutcome::Created(_)) {
            self.signal_inbox(turn.target);
        }
        Ok(outcome)
    }

    pub(super) fn insert_complete_operator_turn(
        conn: &Connection,
        turn: &OperatorTurn<'_>,
        event_id: &str,
    ) -> Result<EnqueueOutcome> {
        let metadata = &turn.metadata;
        let now = Utc::now();
        if let Some(key) = metadata.dedupe_key {
            if let Some(id) = conn
                .query_row(
                    "SELECT id FROM prompt_queue WHERE dedupe_key = ?",
                    [key],
                    |row| row.get(0),
                )
                .optional()?
            {
                // Keep the first writer's attribution and snapshot on replay.
                return Ok(EnqueueOutcome::SuppressedDuplicate(id));
            }
        } else if !metadata.urgent && turn.source != "supervisor" {
            let cutoff =
                (now - chrono::Duration::seconds(PROMPT_DUPLICATE_WINDOW_SECS)).to_rfc3339();
            if let Some(id) = conn.query_row(
                "SELECT id FROM prompt_queue WHERE source = ? AND target = ? AND prompt = ?
                 AND factory_session IS ? AND urgent = 0 AND transport_delivered_at IS NOT NULL
                 AND acked_at IS NULL AND highest_stage IS NOT 'confirmed' AND transport_delivered_at >= ?
                 ORDER BY transport_delivered_at DESC, id DESC LIMIT 1",
                params![turn.source, turn.target, turn.prompt, turn.factory_session, cutoff], |row| row.get(0),
            ).optional()? {
                return Ok(EnqueueOutcome::SuppressedDuplicate(id));
            }
        }
        let stamped_origin = metadata.operator.map(OperatorStamp::origin);
        let origin = stamped_origin.as_ref().or(metadata.origin);
        let operator = metadata.operator;
        let scopes = operator
            .map(|stamp| serde_json::to_string(&stamp.scopes))
            .transpose()?;
        let attachments = serde_json::to_string(metadata.attachments)?;
        let attribution = metadata
            .attribution
            .map(serde_json::to_string)
            .transpose()?;
        let created_at = now.to_rfc3339();
        let priority: i32 = metadata
            .priority
            .unwrap_or(NotificationPriority::Normal)
            .into();
        conn.execute(
            "INSERT INTO prompt_queue (source, target, prompt, created_at, factory_session, summary, priority, urgent,
                attribution_json, origin_agent_id, origin_kind, operator_label, operator_device_id, operator_device_label,
                operator_scopes, operator_verified, recipient_device_id, kind, attachments, dedupe_key)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20)",
            params![turn.source, turn.target, turn.prompt, created_at, turn.factory_session, metadata.summary,
                priority, i64::from(metadata.urgent), attribution, origin.and_then(QueueOrigin::agent_id), origin.map(QueueOrigin::kind_str),
                operator.map(|stamp| &stamp.operator), operator.map(|stamp| &stamp.device_id), operator.map(|stamp| &stamp.device_label),
                scopes, operator.map(|stamp| i64::from(stamp.verified)), metadata.recipient_device_id,
                metadata.kind, attachments, metadata.dedupe_key],
        )?;
        let id = conn.last_insert_rowid();
        if turn.target.trim().eq_ignore_ascii_case("operator")
            || operator.is_some_and(|stamp| stamp.verified)
        {
            let snapshot = serde_json::to_string(&serde_json::json!({
                "schema_version": 1, "event_id": event_id, "prompt_id": id,
                "source": turn.source, "target": turn.target, "prompt": turn.prompt,
                "created_at": created_at, "factory_session": turn.factory_session,
                "summary": metadata.summary, "priority": priority, "urgent": metadata.urgent,
                "origin": origin, "operator": operator, "attribution": metadata.attribution,
                "recipient_device_id": metadata.recipient_device_id, "kind": metadata.kind,
                "attachments": metadata.attachments,
                "acknowledge_prompt_id": metadata.acknowledge_prompt_id,
            }))?;
            conn.execute(
                "INSERT INTO operator_delivery_outbox (event_id,prompt_id,factory_session,payload_snapshot,created_at)
                 VALUES (?1,?2,?3,?4,?5)", params![event_id, id, turn.factory_session, snapshot, created_at],
            )?;
            // cas-9b7d: the enrolled cloud lane, in this same transaction and
            // only while a verified binding is active (no backfill).
            Self::insert_cloud_outbox_row(conn, event_id, turn.factory_session, &created_at)?;
        }
        if let Some(ack_id) = metadata.acknowledge_prompt_id {
            // Missing/already-acked rows remain idempotent. A real stamp/DB
            // failure rolls back the whole reply rather than hiding a partial
            // completion. Do not start a nested stage transaction here.
            let exists: bool = conn.query_row(
                "SELECT EXISTS (SELECT 1 FROM prompt_queue WHERE id = ?)",
                [ack_id],
                |row| row.get(0),
            )?;
            if exists {
                Self::atomic_stage_stamp_in_tx(
                    conn,
                    ack_id,
                    DeliveryStage::Confirmed,
                    AtomicStampOpts::clear_reason(),
                )?;
            }
            conn.execute("UPDATE prompt_queue SET acked_at = ?, acked_via = 'explicit_ack' WHERE id = ? AND acked_at IS NULL", params![created_at, ack_id])?;
        }
        let _ = capture_message_event(conn, turn.source, turn.target);
        Ok(EnqueueOutcome::Created(id))
    }

    pub fn operator_delivery_event(&self, prompt_id: i64) -> Result<Option<OperatorDeliveryEvent>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            "SELECT event_id,prompt_id,factory_session,audience_state,payload_snapshot,created_at
             FROM operator_delivery_outbox WHERE prompt_id = ?",
            [prompt_id],
            Self::operator_event_from_row,
        )
        .optional()
        .map_err(Into::into)
    }

    fn operator_event_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OperatorDeliveryEvent> {
        Ok(OperatorDeliveryEvent {
            event_id: row.get(0)?,
            prompt_id: row.get(1)?,
            factory_session: row.get(2)?,
            audience_state: row.get(3)?,
            payload_snapshot: row.get(4)?,
            created_at: row.get(5)?,
        })
    }

    pub fn claim_operator_delivery(
        &self,
        now: DateTime<Utc>,
        limits: OperatorDrainLimits,
    ) -> Result<Vec<OperatorDeliveryClaim>> {
        limits.validate()?;
        let lease_until = now
            .checked_add_signed(chrono::Duration::seconds(limits.lease_seconds))
            .ok_or_else(|| StoreError::Other("operator lease exceeds timestamp range".into()))?;
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = ImmediateTx::new(&conn)?;
            // Read lengths first; a caller cannot allocate an unbounded batch
            // merely by leaving large local prompt snapshots in this queue.
            let candidates = {
                let mut stmt = tx.prepare("SELECT event_id, length(CAST(payload_snapshot AS BLOB)) FROM operator_delivery_outbox
                    WHERE retained_at IS NULL AND retry_at_ms <= ?1 AND (lease_until_ms IS NULL OR lease_until_ms <= ?1)
                    ORDER BY created_at,event_id LIMIT ?2")?;
                stmt.query_map(
                    params![now.timestamp_millis(), limits.max_events as i64],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
            };
            let mut bytes = 0usize;
            let mut claims = Vec::new();
            for (event_id, length) in candidates {
                let length = usize::try_from(length)
                    .map_err(|_| StoreError::Other("invalid operator snapshot length".into()))?;
                if length > limits.max_payload_bytes && claims.is_empty() {
                    return Err(StoreError::Other(
                        "operator event exceeds the drain byte budget; event remains pending"
                            .into(),
                    ));
                }
                if length > limits.max_payload_bytes.saturating_sub(bytes) {
                    break;
                }
                let event = tx.query_row("SELECT event_id,prompt_id,factory_session,audience_state,payload_snapshot,created_at
                    FROM operator_delivery_outbox WHERE event_id = ?", [&event_id], Self::operator_event_from_row)?;
                let token = random_identity();
                tx.execute("UPDATE operator_delivery_outbox SET lease_token = ?1, lease_until_ms = ?2, attempts = attempts + 1
                    WHERE event_id = ?3", params![token, lease_until.timestamp_millis(), event_id])?;
                bytes += length;
                claims.push(OperatorDeliveryClaim {
                    event,
                    lease_token: token,
                    lease_until,
                });
            }
            tx.commit()?;
            Ok(claims)
        })
    }

    /// A stale lease cannot settle a newer worker's claim. Only a matching
    /// retained receipt ends retry; this does not touch prompt/read ACK state.
    pub fn retain_operator_delivery(
        &self,
        claim: &OperatorDeliveryClaim,
        receipt: &OperatorRelayReceipt,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        if receipt.event_id != claim.event.event_id || receipt.receipt_id.trim().is_empty() {
            return Err(StoreError::Other(
                "operator storage receipt does not match the claimed event".into(),
            ));
        }
        let receipt = serde_json::to_string(receipt)?;
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            Ok(conn.execute("UPDATE operator_delivery_outbox SET relay_receipt = ?1, retained_at = ?2,
                    lease_token = NULL, lease_until_ms = NULL
                WHERE event_id = ?3 AND lease_token = ?4 AND lease_until_ms > ?5 AND retained_at IS NULL
                    AND payload_snapshot = ?6",
                params![receipt, now.to_rfc3339(), claim.event.event_id, claim.lease_token, now.timestamp_millis(), claim.event.payload_snapshot])? > 0)
        })
    }

    pub fn retry_operator_delivery(
        &self,
        claim: &OperatorDeliveryClaim,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = ImmediateTx::new(&conn)?;
            let attempts: Option<u32> = tx.query_row("SELECT attempts FROM operator_delivery_outbox
                WHERE event_id = ?1 AND lease_token = ?2 AND lease_until_ms > ?3 AND retained_at IS NULL",
                params![claim.event.event_id, claim.lease_token, now.timestamp_millis()], |row| row.get(0)).optional()?;
            let Some(attempts) = attempts else {
                return Ok(false);
            };
            let seconds = (1_i64 << attempts.saturating_sub(1).min(6)).min(60);
            let retry_at = now
                .checked_add_signed(chrono::Duration::seconds(seconds))
                .ok_or_else(|| {
                    StoreError::Other("operator retry exceeds timestamp range".into())
                })?;
            tx.execute("UPDATE operator_delivery_outbox SET retry_at_ms = ?1, lease_token = NULL, lease_until_ms = NULL
                WHERE event_id = ?2 AND lease_token = ?3", params![retry_at.timestamp_millis(), claim.event.event_id, claim.lease_token])?;
            tx.commit()?;
            Ok(true)
        })
    }

    /// One bounded batch, with no transport operation under a SQLite lock.
    /// The clock is consulted after each retain to fence elapsed leases too.
    pub fn drain_operator_delivery(
        &self,
        transport: &dyn OperatorDeliveryTransport,
        limits: OperatorDrainLimits,
        clock: impl Fn() -> DateTime<Utc>,
    ) -> Result<OperatorDrainReport> {
        let claims = self.claim_operator_delivery(clock(), limits)?;
        let mut report = OperatorDrainReport {
            claimed: claims.len(),
            ..Default::default()
        };
        for claim in claims {
            match transport.retain(&claim.event) {
                Ok(receipt)
                    if receipt.event_id == claim.event.event_id
                        && !receipt.receipt_id.trim().is_empty() =>
                {
                    if self.retain_operator_delivery(&claim, &receipt, clock())? {
                        report.retained += 1;
                    } else {
                        report.fenced += 1;
                    }
                }
                // Refused/invalid ACKs never retire an event or leak raw
                // transport failures into diagnostics or a persisted prompt.
                _ => {
                    if self.retry_operator_delivery(&claim, clock())? {
                        report.retry_scheduled += 1;
                    } else {
                        report.fenced += 1;
                    }
                }
            }
        }
        Ok(report)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use tempfile::TempDir;

    fn fixture() -> (TempDir, SqlitePromptQueueStore) {
        let dir = TempDir::new().unwrap();
        let store = SqlitePromptQueueStore::open(dir.path()).unwrap();
        store.init().unwrap();
        (dir, store)
    }

    fn reply<'a>(payload: &'a str, key: Option<&'a str>) -> OperatorTurn<'a> {
        static DAEMON: QueueOrigin = QueueOrigin::Daemon;
        OperatorTurn {
            source: "supervisor",
            target: "operator",
            prompt: payload,
            factory_session: Some("session"),
            metadata: OperatorTurnMetadata {
                summary: Some("summary"),
                origin: Some(&DAEMON),
                recipient_device_id: Some("phone"),
                kind: Some("answer"),
                dedupe_key: key,
                ..Default::default()
            },
        }
    }

    #[derive(Default)]
    struct InProcessRelay {
        events: RefCell<HashMap<String, String>>,
        lose_ack: Cell<bool>,
    }

    impl OperatorDeliveryTransport for InProcessRelay {
        fn retain(&self, event: &OperatorDeliveryEvent) -> Result<OperatorRelayReceipt> {
            let mut events = self.events.borrow_mut();
            if let Some(prior) = events.get(&event.event_id) {
                if prior != &event.payload_snapshot {
                    return Err(StoreError::Other("event_conflict".into()));
                }
            } else {
                events.insert(event.event_id.clone(), event.payload_snapshot.clone());
            }
            if self.lose_ack.replace(false) {
                return Err(StoreError::Other("fixture_ack_lost".into()));
            }
            Ok(OperatorRelayReceipt {
                event_id: event.event_id.clone(),
                receipt_id: format!("retained:{}", event.event_id),
            })
        }
    }

    #[test]
    fn complete_reply_and_explicit_ack_commit_with_one_frozen_event() {
        let (_dir, store) = fixture();
        let request = store
            .enqueue("commander", "supervisor", "question")
            .unwrap();
        let mut turn = reply("answer", Some("reply-once"));
        turn.metadata.acknowledge_prompt_id = Some(request);
        let id = store.record_operator_turn(&turn).unwrap().id();
        let row = store.queued_prompt(id).unwrap().unwrap();
        assert_eq!(row.kind.as_deref(), Some("answer"));
        assert_eq!(row.recipient_device_id.as_deref(), Some("phone"));
        assert!(
            store
                .queued_prompt(request)
                .unwrap()
                .unwrap()
                .acked_at
                .is_some()
        );
        let event = store.operator_delivery_event(id).unwrap().unwrap();
        assert_eq!(event.audience_state, "unenrolled");
        let payload: serde_json::Value = serde_json::from_str(&event.payload_snapshot).unwrap();
        assert_eq!(payload["prompt"], "answer");
        assert_eq!(payload["kind"], "answer");
        assert_eq!(payload["recipient_device_id"], "phone");
        store.stamp_operator_reply(id, "status", &[]).unwrap();
        assert_eq!(
            store
                .operator_delivery_event(id)
                .unwrap()
                .unwrap()
                .payload_snapshot,
            event.payload_snapshot,
            "legacy mutable row stamps cannot alter a frozen event"
        );
        assert_eq!(store.record_operator_turn(&turn).unwrap().id(), id);
        assert_eq!(
            store
                .claim_operator_delivery(Utc::now(), OperatorDrainLimits::default())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn outbox_insert_failure_rolls_back_prompt_metadata_and_completion() {
        let (_dir, store) = fixture();
        let request = store
            .enqueue("commander", "supervisor", "question")
            .unwrap();
        crate::shared_db::lock_connection(&store.conn)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fixture_fail_outbox BEFORE INSERT ON operator_delivery_outbox
             BEGIN SELECT RAISE(ABORT, 'fixture precommit crash'); END;",
            )
            .unwrap();
        let mut turn = reply("answer", None);
        turn.metadata.acknowledge_prompt_id = Some(request);
        assert!(store.record_operator_turn(&turn).is_err());
        assert!(
            store
                .queued_prompt(request)
                .unwrap()
                .unwrap()
                .acked_at
                .is_none()
        );
        assert!(
            store
                .peek_operator_replies("session", 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .claim_operator_delivery(Utc::now(), OperatorDrainLimits::default())
                .unwrap()
                .is_empty()
        );
        crate::shared_db::lock_connection(&store.conn)
            .unwrap()
            .execute_batch("DROP TRIGGER fixture_fail_outbox")
            .unwrap();
        let id = store.record_operator_turn(&turn).unwrap().id();
        assert!(store.operator_delivery_event(id).unwrap().is_some());
        assert!(
            store
                .queued_prompt(request)
                .unwrap()
                .unwrap()
                .acked_at
                .is_some()
        );
    }

    #[test]
    fn completion_failure_after_outbox_insert_rolls_back_the_entire_turn() {
        let (_dir, store) = fixture();
        let request = store
            .enqueue("commander", "supervisor", "question")
            .unwrap();
        crate::shared_db::lock_connection(&store.conn)
            .unwrap()
            .execute_batch(
                "CREATE TRIGGER fixture_fail_ack BEFORE UPDATE OF acked_at ON prompt_queue
             BEGIN SELECT RAISE(ABORT, 'fixture after outbox before commit'); END;",
            )
            .unwrap();
        let mut turn = reply("answer", None);
        turn.metadata.acknowledge_prompt_id = Some(request);
        assert!(store.record_operator_turn(&turn).is_err());
        assert!(
            store
                .queued_prompt(request)
                .unwrap()
                .unwrap()
                .acked_at
                .is_none()
        );
        assert!(
            store
                .peek_operator_replies("session", 10)
                .unwrap()
                .is_empty()
        );
        assert!(
            store
                .claim_operator_delivery(Utc::now(), OperatorDrainLimits::default())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn verified_sender_stamps_and_snapshot_are_atomic_without_label_audience() {
        let (_dir, store) = fixture();
        let stamp = OperatorStamp {
            operator: "display only".into(),
            device_id: "device".into(),
            device_label: "phone".into(),
            scopes: vec!["message:send".into()],
            verified: true,
        };
        let id = store
            .enqueue_operator_message(
                "commander:display",
                "supervisor",
                "question",
                Some("session"),
                None,
                None,
                false,
                None,
                &stamp,
            )
            .unwrap()
            .id();
        let row = store.queued_prompt(id).unwrap().unwrap();
        assert_eq!(row.operator.as_ref(), Some(&stamp));
        let event = store.operator_delivery_event(id).unwrap().unwrap();
        let snapshot: serde_json::Value = serde_json::from_str(&event.payload_snapshot).unwrap();
        assert_eq!(snapshot["operator"]["device_id"], "device");
        assert_eq!(snapshot["origin"]["kind"], "paired_device");
        assert_eq!(event.audience_state, "unenrolled");
        store
            .enqueue("worker", "supervisor", "ordinary report")
            .unwrap();
        let unverified = OperatorStamp {
            verified: false,
            ..stamp
        };
        let id = store
            .enqueue_operator_message(
                "commander:display",
                "supervisor",
                "unverified",
                Some("session"),
                None,
                None,
                false,
                None,
                &unverified,
            )
            .unwrap()
            .id();
        assert!(store.operator_delivery_event(id).unwrap().is_none());
        assert_eq!(
            store
                .claim_operator_delivery(Utc::now(), OperatorDrainLimits::default())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn restart_after_commit_before_drain_recovers_same_event_and_lease() {
        let (dir, store) = fixture();
        let id = store
            .record_operator_turn(&reply("answer", None))
            .unwrap()
            .id();
        let before = store.operator_delivery_event(id).unwrap().unwrap();
        let now = Utc::now();
        let first = store
            .claim_operator_delivery(now, OperatorDrainLimits::default())
            .unwrap()
            .pop()
            .unwrap();
        drop(store);
        let reader = SqlitePromptQueueStore::open_read_only(dir.path()).unwrap();
        assert_eq!(
            reader
                .operator_delivery_event(id)
                .unwrap()
                .unwrap()
                .event_id,
            before.event_id
        );
        let reopened = SqlitePromptQueueStore::open(dir.path()).unwrap();
        reopened.init().unwrap();
        assert!(
            reopened
                .claim_operator_delivery(now, OperatorDrainLimits::default())
                .unwrap()
                .is_empty()
        );
        let reclaimed = reopened
            .claim_operator_delivery(first.lease_until, OperatorDrainLimits::default())
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(reclaimed.event.event_id, before.event_id);
        assert_eq!(reclaimed.event.payload_snapshot, before.payload_snapshot);
        assert_ne!(reclaimed.lease_token, first.lease_token);
        let receipt = OperatorRelayReceipt {
            event_id: before.event_id,
            receipt_id: "receipt".into(),
        };
        assert!(
            !reopened
                .retain_operator_delivery(&first, &receipt, first.lease_until)
                .unwrap()
        );
        assert!(
            !reopened
                .retry_operator_delivery(&first, first.lease_until)
                .unwrap()
        );
        assert!(
            reopened
                .retain_operator_delivery(&reclaimed, &receipt, first.lease_until)
                .unwrap()
        );
    }

    #[test]
    fn expired_lease_cannot_accept_a_late_ack_without_reclaim() {
        let (_dir, store) = fixture();
        store.record_operator_turn(&reply("answer", None)).unwrap();
        let claim = store
            .claim_operator_delivery(Utc::now(), OperatorDrainLimits::default())
            .unwrap()
            .pop()
            .unwrap();
        let receipt = OperatorRelayReceipt {
            event_id: claim.event.event_id.clone(),
            receipt_id: "receipt".into(),
        };
        assert!(
            !store
                .retain_operator_delivery(&claim, &receipt, claim.lease_until)
                .unwrap()
        );
        assert_eq!(
            store
                .claim_operator_delivery(claim.lease_until, OperatorDrainLimits::default())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn lost_relay_ack_and_duplicate_drain_retain_exactly_one_event() {
        let (_dir, store) = fixture();
        let id = store
            .record_operator_turn(&reply("answer", None))
            .unwrap()
            .id();
        let relay = InProcessRelay::default();
        relay.lose_ack.set(true);
        let now = Utc::now();
        let first = store
            .drain_operator_delivery(&relay, OperatorDrainLimits::default(), || now)
            .unwrap();
        assert_eq!(first.retry_scheduled, 1);
        assert_eq!(relay.events.borrow().len(), 1);
        assert_eq!(
            store
                .drain_operator_delivery(&relay, OperatorDrainLimits::default(), || now)
                .unwrap()
                .claimed,
            0
        );
        let retry = now + chrono::Duration::seconds(1);
        assert_eq!(
            store
                .drain_operator_delivery(&relay, OperatorDrainLimits::default(), || retry)
                .unwrap()
                .retained,
            1
        );
        assert_eq!(
            store
                .drain_operator_delivery(&relay, OperatorDrainLimits::default(), || retry)
                .unwrap()
                .claimed,
            0
        );
        assert_eq!(relay.events.borrow().len(), 1);
        // Storage ACK is neither prompt delivery nor operator read.
        let prompt = store.queued_prompt(id).unwrap().unwrap();
        assert!(prompt.acked_at.is_none());
        assert!(prompt.processed_at.is_none());
    }

    #[test]
    fn mismatched_receipt_retries_and_time_spent_in_transport_fences_ack() {
        let (_dir, store) = fixture();
        store.record_operator_turn(&reply("answer", None)).unwrap();
        struct WrongReceipt;
        impl OperatorDeliveryTransport for WrongReceipt {
            fn retain(&self, _event: &OperatorDeliveryEvent) -> Result<OperatorRelayReceipt> {
                Ok(OperatorRelayReceipt {
                    event_id: "another-event".into(),
                    receipt_id: "receipt".into(),
                })
            }
        }
        let now = Utc::now();
        assert_eq!(
            store
                .drain_operator_delivery(&WrongReceipt, OperatorDrainLimits::default(), || now)
                .unwrap()
                .retry_scheduled,
            1
        );
        let relay = InProcessRelay::default();
        let call = Cell::new(0);
        let limits = OperatorDrainLimits {
            lease_seconds: 1,
            ..Default::default()
        };
        let report = store
            .drain_operator_delivery(&relay, limits, || {
                let step = call.get();
                call.set(step + 1);
                now + chrono::Duration::seconds(1 + step)
            })
            .unwrap();
        assert_eq!(report.fenced, 1);
        assert_eq!(report.retained, 0);
        assert_eq!(relay.events.borrow().len(), 1);
    }

    #[test]
    fn concurrent_connections_cannot_claim_the_same_unexpired_event() {
        let (dir, store) = fixture();
        store.record_operator_turn(&reply("answer", None)).unwrap();
        // An independent connection, not the process-local shared pool.
        let conn = Connection::open(dir.path().join("cas.db")).unwrap();
        conn.busy_timeout(crate::SQLITE_BUSY_TIMEOUT).unwrap();
        let peer = SqlitePromptQueueStore {
            conn: Arc::new(Mutex::new(conn)),
        };
        let now = Utc::now();
        let (left, right) = std::thread::scope(|scope| {
            let left = scope.spawn(|| {
                store
                    .claim_operator_delivery(now, OperatorDrainLimits::default())
                    .unwrap()
                    .len()
            });
            let right = scope.spawn(|| {
                peer.claim_operator_delivery(now, OperatorDrainLimits::default())
                    .unwrap()
                    .len()
            });
            (left.join().unwrap(), right.join().unwrap())
        });
        assert_eq!(left + right, 1);
    }

    #[test]
    fn batch_byte_and_count_limits_preserve_unclaimed_events() {
        let (_dir, store) = fixture();
        let first = store
            .record_operator_turn(&reply("first", None))
            .unwrap()
            .id();
        store.record_operator_turn(&reply("second", None)).unwrap();
        let now = Utc::now();
        let small = OperatorDrainLimits {
            max_payload_bytes: 1,
            ..Default::default()
        };
        assert!(store.claim_operator_delivery(now, small).is_err());
        let too_many = OperatorDrainLimits {
            max_events: 101,
            ..Default::default()
        };
        assert!(store.claim_operator_delivery(now, too_many).is_err());
        let bytes = store
            .operator_delivery_event(first)
            .unwrap()
            .unwrap()
            .payload_snapshot
            .len();
        let limits = OperatorDrainLimits {
            max_payload_bytes: bytes,
            ..Default::default()
        };
        assert_eq!(store.claim_operator_delivery(now, limits).unwrap().len(), 1);
        assert_eq!(
            store
                .claim_operator_delivery(now, OperatorDrainLimits::default())
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn immutable_event_survives_prompt_mutation_and_automatic_retention() {
        let (_dir, store) = fixture();
        let id = store
            .record_operator_turn(&reply("answer", None))
            .unwrap()
            .id();
        let event = store.operator_delivery_event(id).unwrap().unwrap();
        let old = (Utc::now() - chrono::Duration::days(2)).to_rfc3339();
        {
            let conn = crate::shared_db::lock_connection(&store.conn).unwrap();
            conn.execute(
                "UPDATE prompt_queue SET processed_at = ?1, prompt = 'changed' WHERE id = ?2",
                params![old, id],
            )
            .unwrap();
            assert!(conn.execute("UPDATE operator_delivery_outbox SET payload_snapshot = 'changed' WHERE prompt_id = ?", [id]).is_err());
        }
        assert_eq!(store.cleanup_old(60).unwrap(), 0);
        assert_eq!(store.prune_terminal_older_than(60).unwrap().pruned, 0);
        assert!(store.queued_prompt(id).unwrap().is_some());
        assert_eq!(
            store
                .operator_delivery_event(id)
                .unwrap()
                .unwrap()
                .payload_snapshot,
            event.payload_snapshot
        );
        assert_eq!(store.clear().unwrap(), 1);
        assert!(store.operator_delivery_event(id).unwrap().is_none());
    }

    #[test]
    fn mirror_and_idempotent_watchdog_adapters_write_one_complete_event() {
        let (_dir, store) = fixture();
        let now = Utc::now();
        let payload = r#"{"schema_version":2,"message":"mirrored","device_id":"phone","kind":"status","attachments":[]}"#;
        let id = store
            .mirror_supervisor_turn(
                "session",
                "mirror-key",
                now,
                now,
                payload,
                "summary",
                "phone",
                "status",
            )
            .unwrap()
            .unwrap();
        assert!(
            store
                .mirror_supervisor_turn(
                    "session",
                    "mirror-key",
                    now,
                    now,
                    payload,
                    "summary",
                    "phone",
                    "status"
                )
                .unwrap()
                .is_none()
        );
        let event = store.operator_delivery_event(id).unwrap().unwrap();
        let snapshot: serde_json::Value = serde_json::from_str(&event.payload_snapshot).unwrap();
        assert_eq!(snapshot["kind"], "status");
        assert_eq!(snapshot["recipient_device_id"], "phone");
        let watchdog = store
            .enqueue_idempotent(
                "relay-watchdog",
                "operator",
                "notice",
                Some("session"),
                None,
                None,
                "relay-escalation:1",
                Some(&QueueOrigin::Daemon),
            )
            .unwrap();
        let repeated = store
            .enqueue_idempotent(
                "relay-watchdog",
                "operator",
                "notice",
                Some("session"),
                None,
                None,
                "relay-escalation:1",
                Some(&QueueOrigin::Daemon),
            )
            .unwrap();
        let EnqueueIdempotentResult::Created(watchdog_id) = watchdog else {
            panic!("new notice should be created")
        };
        assert_eq!(
            repeated,
            EnqueueIdempotentResult::AlreadyExists(watchdog_id)
        );
        assert!(
            store
                .operator_delivery_event(watchdog_id)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            store
                .claim_operator_delivery(Utc::now(), OperatorDrainLimits::default())
                .unwrap()
                .len(),
            2
        );
    }
}
