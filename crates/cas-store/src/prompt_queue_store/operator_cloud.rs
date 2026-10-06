//! Enrolled cloud delivery of operator turns (cas-9b7d S2, migration 265).
//!
//! m263 records every operator turn with an immutable, explicitly
//! *unenrolled* local snapshot. This module adds the enrolled lane on top,
//! without relabelling anything m263 recorded:
//!
//! - `operator_feed_binding` is the verified account audience of this
//!   project database: one row, written by `cas hub operator bind` after the
//!   hub's machine enrollment verified its cloud binding. It names the
//!   account, machine, hub and canonical project routing IDs.
//! - `operator_cloud_outbox` gets one row per turn recorded **while a binding
//!   is active**, inside the same IMMEDIATE transaction as the prompt and its
//!   m263 snapshot. Turns recorded before the binding never get a row: there
//!   is no implicit backfill of pre-consent history.
//!
//! Sealing happens outside the SQLite lock (the caller holds the epoch key);
//! the sealed bytes are then compare-and-set into the claimed row, and every
//! retry uploads exactly those bytes. Only a cloud `epoch_retired` refusal
//! (the event was never stored) may clear the seal so the same frozen payload
//! is re-sealed under the new epoch with the same `event_id` (contract §6.5).
//! A receipt (`stored`, `duplicate` or `expired`) ends the row's retry; a
//! terminal refusal parks it with a closed reason for `doctor`.

use super::*;

pub const OPERATOR_CLOUD_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS operator_feed_binding (
        singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
        account_id TEXT NOT NULL CHECK (length(account_id) BETWEEN 1 AND 200),
        machine_id TEXT NOT NULL CHECK (length(machine_id) BETWEEN 1 AND 200),
        hub_id TEXT NOT NULL CHECK (length(hub_id) BETWEEN 1 AND 200),
        project_id TEXT NOT NULL CHECK (length(project_id) BETWEEN 1 AND 200),
        bound_at TEXT NOT NULL,
        detached_at TEXT
    )",
    "CREATE TABLE IF NOT EXISTS operator_cloud_outbox (
        event_id TEXT PRIMARY KEY,
        account_id TEXT NOT NULL,
        machine_id TEXT NOT NULL,
        hub_id TEXT NOT NULL,
        project_id TEXT NOT NULL,
        session_id TEXT NOT NULL,
        created_at TEXT NOT NULL,
        feed_generation TEXT,
        key_epoch TEXT,
        ciphertext TEXT,
        digest TEXT,
        seal_count INTEGER NOT NULL DEFAULT 0,
        attempts INTEGER NOT NULL DEFAULT 0,
        retry_at_ms INTEGER NOT NULL DEFAULT 0,
        lease_token TEXT,
        lease_until_ms INTEGER,
        receipt_kind TEXT CHECK (receipt_kind IN ('stored', 'duplicate', 'expired')),
        sequence TEXT,
        receipt_at TEXT,
        parked_reason TEXT,
        CHECK ((ciphertext IS NULL) = (digest IS NULL)),
        CHECK ((ciphertext IS NULL) = (key_epoch IS NULL)),
        CHECK ((ciphertext IS NULL) = (feed_generation IS NULL)),
        CHECK ((lease_token IS NULL) = (lease_until_ms IS NULL)),
        CHECK ((receipt_kind IS NULL) = (receipt_at IS NULL)),
        CHECK (receipt_kind IS NULL OR ciphertext IS NOT NULL)
    )",
    "CREATE INDEX IF NOT EXISTS idx_operator_cloud_pending
        ON operator_cloud_outbox(retry_at_ms, created_at, event_id)
        WHERE receipt_kind IS NULL AND parked_reason IS NULL",
    "CREATE TRIGGER IF NOT EXISTS operator_cloud_identity_immutable
        BEFORE UPDATE OF event_id, account_id, machine_id, hub_id, project_id, session_id, created_at
        ON operator_cloud_outbox
        WHEN NEW.event_id IS NOT OLD.event_id OR NEW.account_id IS NOT OLD.account_id
          OR NEW.machine_id IS NOT OLD.machine_id OR NEW.hub_id IS NOT OLD.hub_id
          OR NEW.project_id IS NOT OLD.project_id OR NEW.session_id IS NOT OLD.session_id
          OR NEW.created_at IS NOT OLD.created_at
        BEGIN SELECT RAISE(ABORT, 'cloud event identity and audience are immutable'); END",
    "CREATE TRIGGER IF NOT EXISTS operator_cloud_receipt_final
        BEFORE UPDATE ON operator_cloud_outbox
        WHEN OLD.receipt_kind IS NOT NULL AND (NEW.receipt_kind IS NOT OLD.receipt_kind
          OR NEW.ciphertext IS NOT OLD.ciphertext OR NEW.digest IS NOT OLD.digest)
        BEGIN SELECT RAISE(ABORT, 'a cloud receipt is final'); END",
    "CREATE TRIGGER IF NOT EXISTS operator_cloud_follows_local_purge
        AFTER DELETE ON operator_delivery_outbox
        BEGIN DELETE FROM operator_cloud_outbox WHERE event_id = OLD.event_id; END",
    // Offline-command admission (§10.3): one durable queue admission per
    // command ID, committed with its prompt row and the receipt to send.
    "CREATE TABLE IF NOT EXISTS operator_command_admissions (
        command_id TEXT PRIMARY KEY,
        machine_digest TEXT NOT NULL,
        history_event_id TEXT NOT NULL,
        device_id TEXT NOT NULL,
        prompt_id INTEGER NOT NULL UNIQUE,
        admitted_at TEXT NOT NULL,
        receipt_id TEXT NOT NULL UNIQUE,
        receipt_sent_at TEXT
    )",
    "CREATE INDEX IF NOT EXISTS idx_operator_command_receipt_pending
        ON operator_command_admissions(admitted_at) WHERE receipt_sent_at IS NULL",
];

/// A verified, decrypted command ready for admission (§10.3).
pub struct OperatorCommandAdmission<'a> {
    pub command_id: &'a str,
    pub machine_digest: &'a str,
    pub history_event_id: &'a str,
    /// Submitting device from the verified admission authorization (`dev`).
    pub device_id: &'a str,
    pub device_label: &'a str,
    pub factory_session: &'a str,
    pub body: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionOutcome {
    /// Newly admitted: exactly one queue row was written.
    Admitted { prompt_id: i64, receipt_id: String },
    /// The same command was already admitted (crash or duplicate delivery):
    /// no second queue row; resend this receipt if it was not sent.
    Existing {
        prompt_id: i64,
        receipt_id: String,
        receipt_sent: bool,
    },
    /// Same command ID, different payload digest: never admitted twice.
    Conflict,
}

/// The verified audience of this project database.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperatorFeedBinding {
    pub account_id: String,
    pub machine_id: String,
    pub hub_id: String,
    pub project_id: String,
    pub bound_at: String,
}

/// A claimed cloud row with its frozen local snapshot.
#[derive(Clone)]
pub struct OperatorCloudClaim {
    pub event_id: String,
    pub account_id: String,
    pub hub_id: String,
    pub project_id: String,
    pub session_id: String,
    /// The m263 frozen payload (local plaintext JSON); never mutated.
    pub payload_snapshot: String,
    pub factory_session: Option<String>,
    /// Present when the row is already sealed: retries send these bytes.
    pub sealed: Option<OperatorSealedBytes>,
    pub lease_token: String,
    pub attempts: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorSealedBytes {
    pub feed_generation: String,
    pub key_epoch: String,
    /// Base64url ciphertext as uploaded.
    pub ciphertext: String,
    pub digest: String,
}

/// How a claimed row ends this attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperatorCloudSettlement {
    /// The cloud holds it (`stored` or `duplicate`) or tombstoned it (`expired`).
    Receipt {
        kind: &'static str,
        sequence: String,
    },
    /// Retry later with bounded backoff; the sealed bytes stay.
    Retry,
    /// The event was never stored under a retired epoch: clear the seal so the
    /// same payload is re-sealed under the active epoch (§6.5).
    Reseal,
    /// A terminal refusal for this row (closed reason, no free text).
    Park { reason: &'static str },
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct OperatorCloudBacklog {
    pub pending: u64,
    pub parked: u64,
    pub delivered: u64,
    pub oldest_pending_at: Option<String>,
}

/// Opaque, stable session routing ID for a factory session name (D7):
/// `s_` + base64url(SHA-256(UTF-8 name)). The readable name travels only
/// inside the ciphertext.
pub fn session_routing_id(factory_session: &str) -> String {
    use sha2::Digest as _;
    let hash = sha2::Sha256::digest(factory_session.as_bytes());
    format!("s_{}", base64url(&hash))
}

/// Base64url without padding (RFC 4648 §5); cas-store has no base64 crate.
fn base64url(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        let symbols = chunk.len() + 1;
        for index in 0..symbols {
            out.push(char::from(
                ALPHABET[((n >> (18 - 6 * index)) & 63) as usize],
            ));
        }
    }
    out
}

/// The §4.3 routing-ID pattern `^[A-Za-z0-9._:@/-]{1,200}$`.
pub fn is_routing_id(value: &str) -> bool {
    (1..=200).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._:@/-".contains(&b))
}

impl SqlitePromptQueueStore {
    /// Insert the enrolled cloud row for a just-recorded turn, in the caller's
    /// transaction, when (and only when) a binding is active.
    pub(super) fn insert_cloud_outbox_row(
        conn: &Connection,
        event_id: &str,
        factory_session: Option<&str>,
        created_at: &str,
    ) -> Result<()> {
        let binding: Option<(String, String, String, String)> = conn
            .query_row(
                "SELECT account_id, machine_id, hub_id, project_id FROM operator_feed_binding
                 WHERE singleton = 1 AND detached_at IS NULL",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .optional()?;
        let (Some((account, machine, hub, project)), Some(session)) = (binding, factory_session)
        else {
            return Ok(());
        };
        conn.execute(
            "INSERT INTO operator_cloud_outbox (event_id, account_id, machine_id, hub_id, project_id, session_id, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![event_id, account, machine, hub, project, session_routing_id(session), created_at],
        )?;
        Ok(())
    }

    /// Bind this project database to a verified account audience. Re-binding
    /// to the same audience is a no-op; another audience needs `detach` first,
    /// and its pending rows keep their original audience.
    pub fn bind_operator_feed(&self, binding: &OperatorFeedBinding) -> Result<OperatorFeedBinding> {
        for value in [
            &binding.account_id,
            &binding.machine_id,
            &binding.hub_id,
            &binding.project_id,
        ] {
            if !is_routing_id(value) {
                return Err(StoreError::Other(
                    "operator feed binding needs opaque routing IDs (A-Z a-z 0-9 . _ : @ / -)"
                        .into(),
                ));
            }
        }
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = ImmediateTx::new(&conn)?;
            let existing: Option<(OperatorFeedBinding, Option<String>)> = tx
                .query_row(
                    "SELECT account_id, machine_id, hub_id, project_id, bound_at, detached_at
                     FROM operator_feed_binding WHERE singleton = 1",
                    [],
                    |row| {
                        Ok((
                            OperatorFeedBinding {
                                account_id: row.get(0)?,
                                machine_id: row.get(1)?,
                                hub_id: row.get(2)?,
                                project_id: row.get(3)?,
                                bound_at: row.get(4)?,
                            },
                            row.get(5)?,
                        ))
                    },
                )
                .optional()?;
            let result = match existing {
                Some((current, None))
                    if current.account_id == binding.account_id
                        && current.machine_id == binding.machine_id
                        && current.hub_id == binding.hub_id
                        && current.project_id == binding.project_id =>
                {
                    current
                }
                Some((_, None)) => {
                    return Err(StoreError::Other(
                        "this project is bound to another operator audience; detach it first"
                            .into(),
                    ));
                }
                _ => {
                    tx.execute(
                        "INSERT INTO operator_feed_binding (singleton, account_id, machine_id, hub_id, project_id, bound_at, detached_at)
                         VALUES (1, ?1, ?2, ?3, ?4, ?5, NULL)
                         ON CONFLICT(singleton) DO UPDATE SET account_id = excluded.account_id,
                           machine_id = excluded.machine_id, hub_id = excluded.hub_id,
                           project_id = excluded.project_id, bound_at = excluded.bound_at, detached_at = NULL",
                        params![binding.account_id, binding.machine_id, binding.hub_id, binding.project_id, binding.bound_at],
                    )?;
                    binding.clone()
                }
            };
            tx.commit()?;
            Ok(result)
        })
    }

    /// Stop recording new turns for the cloud. Pending rows keep their
    /// audience and still drain to it; nothing is relabelled.
    pub fn detach_operator_feed(&self, now: DateTime<Utc>) -> Result<bool> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            Ok(conn.execute(
                "UPDATE operator_feed_binding SET detached_at = ?1 WHERE singleton = 1 AND detached_at IS NULL",
                [now.to_rfc3339()],
            )? > 0)
        })
    }

    pub fn operator_feed_binding(&self) -> Result<Option<OperatorFeedBinding>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            "SELECT account_id, machine_id, hub_id, project_id, bound_at FROM operator_feed_binding
             WHERE singleton = 1 AND detached_at IS NULL",
            [],
            |row| {
                Ok(OperatorFeedBinding {
                    account_id: row.get(0)?,
                    machine_id: row.get(1)?,
                    hub_id: row.get(2)?,
                    project_id: row.get(3)?,
                    bound_at: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
    }

    /// Claim up to `max_events` due rows for `machine_id` under a lease.
    pub fn claim_operator_cloud(
        &self,
        machine_id: &str,
        now: DateTime<Utc>,
        max_events: usize,
        lease_seconds: i64,
    ) -> Result<Vec<OperatorCloudClaim>> {
        if !(1..=100).contains(&max_events) || !(1..=120).contains(&lease_seconds) {
            return Err(StoreError::Other(
                "operator cloud claim exceeds the bounded batch/lease policy".into(),
            ));
        }
        let lease_until = now.timestamp_millis() + lease_seconds * 1000;
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = ImmediateTx::new(&conn)?;
            let rows = {
                let mut stmt = tx.prepare(
                    "SELECT c.event_id, c.account_id, c.hub_id, c.project_id, c.session_id,
                            o.payload_snapshot, o.factory_session,
                            c.feed_generation, c.key_epoch, c.ciphertext, c.digest, c.attempts
                     FROM operator_cloud_outbox c
                     JOIN operator_delivery_outbox o ON o.event_id = c.event_id
                     WHERE c.machine_id = ?1 AND c.receipt_kind IS NULL AND c.parked_reason IS NULL
                       AND c.retry_at_ms <= ?2 AND (c.lease_until_ms IS NULL OR c.lease_until_ms <= ?2)
                     ORDER BY c.created_at, c.event_id LIMIT ?3",
                )?;
                stmt.query_map(
                    params![machine_id, now.timestamp_millis(), max_events as i64],
                    |row| {
                        let ciphertext: Option<String> = row.get(9)?;
                        let sealed = match ciphertext {
                            Some(ciphertext) => Some(OperatorSealedBytes {
                                feed_generation: row.get(7)?,
                                key_epoch: row.get(8)?,
                                ciphertext,
                                digest: row.get(10)?,
                            }),
                            None => None,
                        };
                        Ok(OperatorCloudClaim {
                            event_id: row.get(0)?,
                            account_id: row.get(1)?,
                            hub_id: row.get(2)?,
                            project_id: row.get(3)?,
                            session_id: row.get(4)?,
                            payload_snapshot: row.get(5)?,
                            factory_session: row.get(6)?,
                            sealed,
                            lease_token: String::new(),
                            attempts: row.get::<_, u32>(11)? + 1,
                        })
                    },
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?
            };
            let mut claims = Vec::with_capacity(rows.len());
            for mut claim in rows {
                let token = super::operator_delivery::new_event_identity();
                tx.execute(
                    "UPDATE operator_cloud_outbox SET lease_token = ?1, lease_until_ms = ?2, attempts = attempts + 1
                     WHERE event_id = ?3",
                    params![token, lease_until, claim.event_id],
                )?;
                claim.lease_token = token;
                claims.push(claim);
            }
            tx.commit()?;
            Ok(claims)
        })
    }

    /// Store sealed bytes for an unsealed claimed row (compare-and-set: a
    /// row already sealed, or a lost lease, refuses).
    pub fn seal_operator_cloud(
        &self,
        claim: &OperatorCloudClaim,
        sealed: &OperatorSealedBytes,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            Ok(conn.execute(
                "UPDATE operator_cloud_outbox SET feed_generation = ?1, key_epoch = ?2, ciphertext = ?3,
                    digest = ?4, seal_count = seal_count + 1
                 WHERE event_id = ?5 AND lease_token = ?6 AND lease_until_ms > ?7
                   AND ciphertext IS NULL AND receipt_kind IS NULL",
                params![
                    sealed.feed_generation,
                    sealed.key_epoch,
                    sealed.ciphertext,
                    sealed.digest,
                    claim.event_id,
                    claim.lease_token,
                    now.timestamp_millis()
                ],
            )? > 0)
        })
    }

    /// Settle a claimed row. A stale lease settles nothing (returns false).
    pub fn settle_operator_cloud(
        &self,
        claim: &OperatorCloudClaim,
        settlement: &OperatorCloudSettlement,
        now: DateTime<Utc>,
    ) -> Result<bool> {
        let now_ms = now.timestamp_millis();
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let changed = match settlement {
                OperatorCloudSettlement::Receipt { kind, sequence } => conn.execute(
                    "UPDATE operator_cloud_outbox SET receipt_kind = ?1, sequence = ?2, receipt_at = ?3,
                        lease_token = NULL, lease_until_ms = NULL
                     WHERE event_id = ?4 AND lease_token = ?5 AND lease_until_ms > ?6
                       AND ciphertext IS NOT NULL AND receipt_kind IS NULL",
                    params![kind, sequence, now.to_rfc3339(), claim.event_id, claim.lease_token, now_ms],
                )?,
                OperatorCloudSettlement::Retry => {
                    let seconds = (1_i64 << claim.attempts.saturating_sub(1).min(6)).min(60);
                    // Deterministic jitter from the event ID spreads a burst.
                    let jitter = i64::from(claim.event_id.bytes().fold(0u8, |a, b| a ^ b)) % 1000;
                    conn.execute(
                        "UPDATE operator_cloud_outbox SET retry_at_ms = ?1, lease_token = NULL, lease_until_ms = NULL
                         WHERE event_id = ?2 AND lease_token = ?3 AND lease_until_ms > ?4",
                        params![now_ms + seconds * 1000 + jitter, claim.event_id, claim.lease_token, now_ms],
                    )?
                }
                OperatorCloudSettlement::Reseal => conn.execute(
                    "UPDATE operator_cloud_outbox SET feed_generation = NULL, key_epoch = NULL, ciphertext = NULL,
                        digest = NULL, retry_at_ms = 0, lease_token = NULL, lease_until_ms = NULL
                     WHERE event_id = ?1 AND lease_token = ?2 AND lease_until_ms > ?3 AND receipt_kind IS NULL",
                    params![claim.event_id, claim.lease_token, now_ms],
                )?,
                OperatorCloudSettlement::Park { reason } => conn.execute(
                    "UPDATE operator_cloud_outbox SET parked_reason = ?1, lease_token = NULL, lease_until_ms = NULL
                     WHERE event_id = ?2 AND lease_token = ?3 AND lease_until_ms > ?4 AND receipt_kind IS NULL",
                    params![reason, claim.event_id, claim.lease_token, now_ms],
                )?,
            };
            Ok(changed > 0)
        })
    }

    /// Admit one offline command: its prompt row for the session's
    /// supervisor, attributed to the submitting device, and the admission
    /// with the receipt ID to send, in one IMMEDIATE transaction. The turn
    /// records no new operator event: its account history already exists
    /// as the command's `history_event`.
    pub fn admit_operator_command(
        &self,
        admission: &OperatorCommandAdmission<'_>,
    ) -> Result<AdmissionOutcome> {
        if admission.body.trim().is_empty() || admission.factory_session.trim().is_empty() {
            return Err(StoreError::Other(
                "an offline command needs a body and a session".into(),
            ));
        }
        let receipt_id = super::operator_delivery::new_event_identity();
        let source = format!("commander:{}", admission.device_id);
        let stamp = OperatorStamp {
            operator: "operator".into(),
            device_id: admission.device_id.to_owned(),
            device_label: admission.device_label.to_owned(),
            scopes: vec!["message:send".into()],
            verified: true,
        };
        // A per-command dedupe key keeps the time-window duplicate filter
        // from swallowing a repeated short reply ("yes" twice is two commands).
        let dedupe_key = format!("operator-command:{}", admission.command_id);
        let attribution = serde_json::json!({
            "via": "operator_inbox_command",
            "command_id": admission.command_id,
            "history_event_id": admission.history_event_id,
        });
        let outcome = crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            let tx = ImmediateTx::new(&conn)?;
            let existing: Option<(String, i64, String, Option<String>)> = tx
                .query_row(
                    "SELECT machine_digest, prompt_id, receipt_id, receipt_sent_at
                     FROM operator_command_admissions WHERE command_id = ?1",
                    [admission.command_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                )
                .optional()?;
            if let Some((digest, prompt_id, receipt_id, sent)) = existing {
                return Ok(if digest == admission.machine_digest {
                    AdmissionOutcome::Existing {
                        prompt_id,
                        receipt_id,
                        receipt_sent: sent.is_some(),
                    }
                } else {
                    AdmissionOutcome::Conflict
                });
            }
            let turn = OperatorTurn {
                source: &source,
                target: "supervisor",
                prompt: admission.body,
                factory_session: Some(admission.factory_session),
                metadata: OperatorTurnMetadata {
                    attribution: Some(&attribution),
                    operator: Some(&stamp),
                    kind: Some("operator_message"),
                    dedupe_key: Some(&dedupe_key),
                    cloud_history_event_id: Some(admission.history_event_id),
                    ..Default::default()
                },
            };
            let event_id = super::operator_delivery::new_event_identity();
            let prompt_id = match Self::insert_complete_operator_turn(&tx, &turn, &event_id)? {
                EnqueueOutcome::Created(id) => id,
                // A dedupe suppression would silently drop a command; refuse.
                _ => {
                    return Err(StoreError::Other(
                        "offline command was suppressed as a duplicate prompt".into(),
                    ));
                }
            };
            tx.execute(
                "INSERT INTO operator_command_admissions
                    (command_id, machine_digest, history_event_id, device_id, prompt_id, admitted_at, receipt_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    admission.command_id,
                    admission.machine_digest,
                    admission.history_event_id,
                    admission.device_id,
                    prompt_id,
                    Utc::now().to_rfc3339(),
                    receipt_id
                ],
            )?;
            tx.commit()?;
            Ok(AdmissionOutcome::Admitted {
                prompt_id,
                receipt_id: receipt_id.clone(),
            })
        })?;
        if matches!(outcome, AdmissionOutcome::Admitted { .. }) {
            self.signal_inbox("supervisor");
        }
        Ok(outcome)
    }

    /// The cloud accepted (or already holds) this command's receipt.
    pub fn mark_command_receipt_sent(&self, command_id: &str, now: DateTime<Utc>) -> Result<bool> {
        crate::shared_db::with_write_retry(|| {
            let conn = crate::shared_db::lock_connection(&self.conn)?;
            Ok(conn.execute(
                "UPDATE operator_command_admissions SET receipt_sent_at = ?1
                 WHERE command_id = ?2 AND receipt_sent_at IS NULL",
                params![now.to_rfc3339(), command_id],
            )? > 0)
        })
    }

    /// `history_event_id` of an admitted prompt, so direct history can carry
    /// the account event ID and devices dedupe the two copies.
    pub fn command_history_event(&self, prompt_id: i64) -> Result<Option<String>> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            "SELECT history_event_id FROM operator_command_admissions WHERE prompt_id = ?1",
            [prompt_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(Into::into)
    }

    /// Backlog age and counts for `doctor` and the hub status (no silent loss).
    pub fn operator_cloud_backlog(&self) -> Result<OperatorCloudBacklog> {
        let conn = crate::shared_db::lock_connection(&self.conn)?;
        conn.query_row(
            "SELECT
                COALESCE(SUM(receipt_kind IS NULL AND parked_reason IS NULL), 0),
                COALESCE(SUM(parked_reason IS NOT NULL), 0),
                COALESCE(SUM(receipt_kind IS NOT NULL), 0),
                MIN(CASE WHEN receipt_kind IS NULL AND parked_reason IS NULL THEN created_at END)
             FROM operator_cloud_outbox",
            [],
            |row| {
                Ok(OperatorCloudBacklog {
                    pending: row.get::<_, i64>(0)? as u64,
                    parked: row.get::<_, i64>(1)? as u64,
                    delivered: row.get::<_, i64>(2)? as u64,
                    oldest_pending_at: row.get(3)?,
                })
            },
        )
        .map_err(Into::into)
    }
}

#[cfg(test)]
#[path = "operator_cloud_tests.rs"]
mod tests;
