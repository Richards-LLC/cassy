//! Reminder storage for factory "Remind Me" feature
//!
//! Allows agents to schedule one-shot reminders that fire after a time delay
//! or when a specific DirectorEvent occurs. The factory daemon checks pending
//! reminders on each tick and delivers them via the prompt queue for PTY
//! injection into the target agent's session.

use chrono::{DateTime, TimeZone, Utc};
use rusqlite::{Connection, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex, TryLockError};
use std::time::Duration;

use crate::Result;
use crate::error::StoreError;

/// How a reminder is triggered
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReminderTriggerType {
    /// Fire after a time delay
    Time,
    /// Fire when a matching DirectorEvent occurs
    Event,
}

impl std::fmt::Display for ReminderTriggerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Time => write!(f, "time"),
            Self::Event => write!(f, "event"),
        }
    }
}

impl std::str::FromStr for ReminderTriggerType {
    type Err = StoreError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "time" => Ok(Self::Time),
            "event" => Ok(Self::Event),
            _ => Err(StoreError::Parse(format!("Unknown trigger type: {s}"))),
        }
    }
}

/// Reminder lifecycle status
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReminderStatus {
    /// Waiting to fire
    Pending,
    /// Successfully delivered
    Fired,
    /// Cancelled by owner
    Cancelled,
    /// TTL exceeded without firing
    Expired,
}

/// Result of a daemon-budgeted stale-reminder expiry attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderExpiryOutcome {
    /// Expiry acquired the database within budget and updated this many rows.
    Expired(usize),
    /// The database was busy; the daemon should retry on its next tick.
    DeferredBusy,
}

impl std::fmt::Display for ReminderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Fired => write!(f, "fired"),
            Self::Cancelled => write!(f, "cancelled"),
            Self::Expired => write!(f, "expired"),
        }
    }
}

impl std::str::FromStr for ReminderStatus {
    type Err = StoreError;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "pending" => Ok(Self::Pending),
            "fired" => Ok(Self::Fired),
            "cancelled" => Ok(Self::Cancelled),
            "expired" => Ok(Self::Expired),
            _ => Err(StoreError::Parse(format!("Unknown reminder status: {s}"))),
        }
    }
}

/// A scheduled reminder
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reminder {
    /// Unique reminder ID
    pub id: i64,
    /// Agent ID of whoever created this reminder
    pub owner_id: String,
    /// Agent ID of who should receive the reminder (may differ from owner
    /// when a supervisor sets a reminder for a worker)
    pub target_id: String,
    /// Message to deliver when fired
    pub message: String,
    /// How this reminder is triggered
    pub trigger_type: ReminderTriggerType,
    /// When to fire (time-based triggers only)
    pub trigger_at: Option<DateTime<Utc>>,
    /// Event type to match (event-based triggers only, e.g. "task_completed")
    pub trigger_event: Option<String>,
    /// JSON subset filter for event matching (e.g. {"task_id":"cas-a1b2"})
    pub trigger_filter: Option<serde_json::Value>,
    /// Current status
    pub status: ReminderStatus,
    /// Time-to-live in seconds before auto-expiry
    pub ttl_secs: i64,
    /// When the reminder was created
    pub created_at: DateTime<Utc>,
    /// When the reminder fired (if it did)
    pub fired_at: Option<DateTime<Utc>>,
    /// When the reminder was cancelled (if it was)
    pub cancelled_at: Option<DateTime<Utc>>,
    /// JSON snapshot of the DirectorEvent that triggered this reminder
    /// (only set for event-based reminders after firing)
    pub fired_event: Option<serde_json::Value>,
    /// Factory session that registered this reminder (`CAS_FACTORY_SESSION`).
    /// Event-based fires only match in this session (cas-fcd4 multi-factory).
    /// `None` = legacy / non-factory registration (any session may fire).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Session that created this reminder (`CAS_SESSION_ID`). Session-scoped
    /// reminders are cancelled when this session ends.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_session_id: Option<String>,
    /// Explicit opt-in for a reminder to survive its creator's SessionEnd.
    /// Such deliveries carry origin and age context for stale-context triage.
    #[serde(default)]
    pub cross_session: bool,
}

/// Wire prefix that marks a prompt-queue row as a fired-reminder delivery.
///
/// The prompt queue has no message-type column, so this prefix IS the
/// discriminator between "the daemon delivered a reminder" and "somebody sent
/// this worker a message" — and those two mean opposite things about a silent
/// worker (cas-f08d / GH #147). It lives here, beside the pair of functions
/// that write and read it, so the producer and every consumer share one
/// definition: changing the wording breaks compilation, not behaviour.
///
/// The durable fix is a message-type column on `prompt_queue`; that is a schema
/// migration and is deliberately out of scope for cas-f08d.
const REMINDER_DELIVERY_PREFIX: &str = "Reminder #";

/// Render a fired reminder as the prompt injected into the target's session.
///
/// `event_context` is the description of the DirectorEvent that fired an
/// event-based reminder; time-based reminders pass `None`.
pub fn format_reminder_delivery(id: i64, message: &str, event_context: Option<&str>) -> String {
    match event_context {
        Some(event) => format!("{REMINDER_DELIVERY_PREFIX}{id}: {message} (triggered by: {event})"),
        None => format!("{REMINDER_DELIVERY_PREFIX}{id}: {message}"),
    }
}

/// Render an explicitly cross-session delivery without losing the durable
/// `Reminder #<id>:` prefix that downstream status logic parses.
pub fn format_cross_session_reminder_delivery(
    reminder: &Reminder,
    event_context: Option<&str>,
) -> String {
    let mut delivery = format_reminder_delivery(reminder.id, &reminder.message, event_context);
    let origin = reminder
        .origin_session_id
        .as_deref()
        .unwrap_or("unknown legacy session");
    delivery.push_str(&format!(
        "\n\n⚠ Cross-session reminder\nOrigin session: {origin}\nCreated at: {}",
        reminder.created_at.to_rfc3339()
    ));
    delivery
}

/// The reminder ID behind a delivery produced by [`format_reminder_delivery`],
/// or `None` when the prompt is ordinary mail.
///
/// The digits and the colon are both required: a message that merely opens with
/// the word "Reminder" is real mail, and treating it as a delivery would revive
/// exactly the false positive this pair exists to kill.
pub fn parse_reminder_delivery_id(prompt: &str) -> Option<i64> {
    let rest = prompt.trim_start().strip_prefix(REMINDER_DELIVERY_PREFIX)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !rest[digits.len()..].starts_with(':') {
        return None;
    }
    digits.parse().ok()
}

/// Schema for reminders table
const REMINDER_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS reminders (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    supervisor_id TEXT NOT NULL,
    message TEXT NOT NULL,
    trigger_type TEXT NOT NULL,
    trigger_at TEXT,
    trigger_event TEXT,
    trigger_filter TEXT,
    status TEXT NOT NULL DEFAULT 'pending',
    ttl_secs INTEGER NOT NULL DEFAULT 3600,
    created_at TEXT NOT NULL DEFAULT (datetime('now')),
    fired_at TEXT,
    cancelled_at TEXT
);

CREATE INDEX IF NOT EXISTS idx_reminders_pending
    ON reminders(supervisor_id, status) WHERE status = 'pending';
CREATE INDEX IF NOT EXISTS idx_reminders_trigger_at
    ON reminders(trigger_at) WHERE status = 'pending' AND trigger_type = 'time';
"#;

/// Migration to add target_id column (separate from supervisor_id/owner)
const MIGRATION_TARGET_ID: &str = "ALTER TABLE reminders ADD COLUMN target_id TEXT";

/// Migration to add fired_event column (stores triggering event JSON)
const MIGRATION_FIRED_EVENT: &str = "ALTER TABLE reminders ADD COLUMN fired_event TEXT";

/// Migration: factory session scope for event-based multi-session isolation (cas-fcd4)
const MIGRATION_SESSION_ID: &str = "ALTER TABLE reminders ADD COLUMN session_id TEXT";
/// Migration: creator-session lifecycle and explicit cross-session opt-in.
const MIGRATION_ORIGIN_SESSION_ID: &str = "ALTER TABLE reminders ADD COLUMN origin_session_id TEXT";
const MIGRATION_CROSS_SESSION: &str =
    "ALTER TABLE reminders ADD COLUMN cross_session INTEGER NOT NULL DEFAULT 0";

/// Trait for reminder storage operations
#[allow(clippy::too_many_arguments)]
pub trait ReminderStore: Send + Sync {
    /// Initialize the store (create tables)
    fn init(&self) -> Result<()>;

    /// Create a new reminder, returns its ID.
    /// `target_id` is the agent who should receive the reminder. If `None`,
    /// defaults to `owner_id` (self-reminder).
    /// `session_id` is the registering factory session (`CAS_FACTORY_SESSION`);
    /// event-based delivery is scoped to that session (cas-fcd4).
    fn create(
        &self,
        owner_id: &str,
        target_id: Option<&str>,
        message: &str,
        trigger_type: ReminderTriggerType,
        trigger_at: Option<DateTime<Utc>>,
        trigger_event: Option<&str>,
        trigger_filter: Option<&serde_json::Value>,
        ttl_secs: i64,
        session_id: Option<&str>,
    ) -> Result<i64>;

    /// Create a reminder with creator-session lifecycle metadata. Callers that
    /// omit this method retain legacy creation semantics through [`Self::create`].
    #[allow(clippy::too_many_arguments)]
    fn create_with_scope(
        &self,
        owner_id: &str,
        target_id: Option<&str>,
        message: &str,
        trigger_type: ReminderTriggerType,
        trigger_at: Option<DateTime<Utc>>,
        trigger_event: Option<&str>,
        trigger_filter: Option<&serde_json::Value>,
        ttl_secs: i64,
        session_id: Option<&str>,
        origin_session_id: Option<&str>,
        cross_session: bool,
    ) -> Result<i64>;

    /// List pending reminders owned by a specific agent
    fn list_pending(&self, owner_id: &str) -> Result<Vec<Reminder>>;

    /// List pending reminders targeting a specific agent (set by others)
    fn list_pending_for_target(&self, target_id: &str) -> Result<Vec<Reminder>>;

    /// List all pending reminders (across all agents)
    fn list_all_pending(&self) -> Result<Vec<Reminder>>;

    /// List reminders fired within the last N seconds (across all agents)
    fn list_recently_fired(&self, within_secs: i64) -> Result<Vec<Reminder>>;

    /// Get pending time-based reminders that are due (trigger_at <= now)
    fn get_due_time_reminders(&self) -> Result<Vec<Reminder>>;

    /// Get pending event-based reminders matching a specific event type
    fn get_event_reminders(&self, event_type: &str) -> Result<Vec<Reminder>>;

    /// Mark a reminder as fired, optionally recording the triggering event
    fn mark_fired(&self, id: i64, fired_event: Option<&serde_json::Value>) -> Result<()>;

    /// Cancel a reminder (must belong to the given owner)
    fn cancel(&self, id: i64, owner_id: &str) -> Result<()>;

    /// Permanently cancel every pending reminder that targets an agent.
    ///
    /// Used by the factory worker-hold gate: a hold is a deliberate pause,
    /// so pre-armed one-shot wakeups must not fire through it or revive later
    /// with stale context after release.
    fn cancel_pending_for_target(&self, target_id: &str) -> Result<usize>;

    /// Cancel pending default-scoped reminders created by a completed session.
    /// Explicit cross-session reminders are deliberately retained.
    fn cancel_pending_for_origin_session(&self, origin_session_id: &str) -> Result<usize>;

    /// Expire reminders past their TTL, returns count expired
    fn expire_stale(&self) -> Result<usize>;

    /// Delete non-pending reminders older than the given number of days
    fn prune(&self, older_than_days: i64) -> Result<usize>;

    /// Close the store
    fn close(&self) -> Result<()>;
}

/// SQLite-based reminder store
pub struct SqliteReminderStore {
    conn: Arc<Mutex<Connection>>,
}

impl SqliteReminderStore {
    /// Open or create a SQLite reminder store
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

    fn reminder_from_row(row: &rusqlite::Row) -> rusqlite::Result<Reminder> {
        let trigger_type_str: String = row.get(3)?;
        let trigger_type = trigger_type_str
            .parse::<ReminderTriggerType>()
            .unwrap_or(ReminderTriggerType::Time);

        let trigger_at_str: Option<String> = row.get(4)?;
        let trigger_at = trigger_at_str.and_then(|s| Self::parse_datetime(&s));

        let trigger_filter_str: Option<String> = row.get(6)?;
        let trigger_filter = trigger_filter_str.and_then(|s| serde_json::from_str(&s).ok());

        let status_str: String = row.get(7)?;
        let status = status_str
            .parse::<ReminderStatus>()
            .unwrap_or(ReminderStatus::Pending);

        let fired_at_str: Option<String> = row.get(10)?;
        let fired_at = fired_at_str.and_then(|s| Self::parse_datetime(&s));

        let cancelled_at_str: Option<String> = row.get(11)?;
        let cancelled_at = cancelled_at_str.and_then(|s| Self::parse_datetime(&s));

        // Column 12 is target_id (may be NULL for old rows)
        let owner_id: String = row.get(1)?;
        let target_id: Option<String> = row.get(12)?;

        // Column 13 is fired_event (may be NULL)
        let fired_event_str: Option<String> = row.get(13)?;
        let fired_event = fired_event_str.and_then(|s| serde_json::from_str(&s).ok());

        // Column 14 is session_id (may be NULL for pre-cas-fcd4 rows)
        let session_id: Option<String> = row.get(14)?;
        let session_id = session_id.and_then(|s| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        });

        let origin_session_id: Option<String> = row.get(15)?;
        let origin_session_id = origin_session_id.and_then(|s| {
            let t = s.trim();
            if t.is_empty() {
                None
            } else {
                Some(t.to_string())
            }
        });
        let cross_session: bool = row.get::<_, i64>(16)? != 0;

        Ok(Reminder {
            id: row.get(0)?,
            target_id: target_id.unwrap_or_else(|| owner_id.clone()),
            owner_id,
            message: row.get(2)?,
            trigger_type,
            trigger_at,
            trigger_event: row.get(5)?,
            trigger_filter,
            status,
            ttl_secs: row.get(8)?,
            created_at: Self::parse_datetime(&row.get::<_, String>(9)?).unwrap_or_else(Utc::now),
            fired_at,
            cancelled_at,
            fired_event,
            session_id,
            origin_session_id,
            cross_session,
        })
    }

    /// Run migrations to add new columns if missing
    fn migrate(&self, conn: &Connection) -> Result<()> {
        if conn
            .prepare_cached("SELECT target_id FROM reminders LIMIT 0")
            .is_err()
        {
            conn.execute_batch(MIGRATION_TARGET_ID)?;
        }

        if conn
            .prepare_cached("SELECT fired_event FROM reminders LIMIT 0")
            .is_err()
        {
            conn.execute_batch(MIGRATION_FIRED_EVENT)?;
        }

        if conn
            .prepare_cached("SELECT session_id FROM reminders LIMIT 0")
            .is_err()
        {
            conn.execute_batch(MIGRATION_SESSION_ID)?;
        }

        if conn
            .prepare_cached("SELECT origin_session_id FROM reminders LIMIT 0")
            .is_err()
        {
            conn.execute_batch(MIGRATION_ORIGIN_SESSION_ID)?;
        }

        if conn
            .prepare_cached("SELECT cross_session FROM reminders LIMIT 0")
            .is_err()
        {
            conn.execute_batch(MIGRATION_CROSS_SESSION)?;
        }

        Ok(())
    }

    const SELECT_COLUMNS: &str = "id, supervisor_id, message, trigger_type, trigger_at, trigger_event, trigger_filter, status, ttl_secs, created_at, fired_at, cancelled_at, target_id, fired_event, session_id, origin_session_id, cross_session";
}

fn expire_stale_with_conn(conn: &Connection) -> Result<usize> {
    // Expire pending reminders where created_at + ttl_secs < now.
    // Use datetime('now') on the RHS so both sides of the comparison use
    // SQLite's canonical 'YYYY-MM-DD HH:MM:SS' format. Previously the RHS
    // was RFC 3339, whose 'T' separator made every reminder compare stale.
    let rows = conn.execute(
        "UPDATE reminders SET status = 'expired'
         WHERE status = 'pending'
         AND datetime(created_at, '+' || ttl_secs || ' seconds') < datetime('now')",
        [],
    )?;

    Ok(rows)
}

/// Expire stale reminders without letting database contention stall a daemon tick.
///
/// In-process connection contention defers immediately. External SQLite write
/// contention waits at most `busy_budget`; a busy result is returned as
/// [`ReminderExpiryOutcome::DeferredBusy`] so the daemon can retry next tick.
/// The shared connection's normal busy timeout is restored before returning.
pub fn expire_stale_bounded(
    cas_dir: &Path,
    busy_budget: Duration,
) -> Result<ReminderExpiryOutcome> {
    let conn = crate::shared_db::shared_connection(&cas_dir.join("cas.db"))?;
    let conn = match conn.try_lock() {
        Ok(conn) => conn,
        Err(TryLockError::WouldBlock) => return Ok(ReminderExpiryOutcome::DeferredBusy),
        Err(TryLockError::Poisoned(poisoned)) => poisoned.into_inner(),
    };

    conn.busy_timeout(busy_budget)?;
    let expiry_result = expire_stale_with_conn(&conn);
    conn.busy_timeout(crate::SQLITE_BUSY_TIMEOUT)?;

    match expiry_result {
        Ok(expired) => Ok(ReminderExpiryOutcome::Expired(expired)),
        Err(StoreError::Database(ref error)) if crate::shared_db::is_busy_error(error) => {
            Ok(ReminderExpiryOutcome::DeferredBusy)
        }
        Err(error) => Err(error),
    }
}

impl ReminderStore for SqliteReminderStore {
    fn init(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        conn.execute_batch(REMINDER_SCHEMA)?;
        self.migrate(&conn)?;
        Ok(())
    }

    fn create(
        &self,
        owner_id: &str,
        target_id: Option<&str>,
        message: &str,
        trigger_type: ReminderTriggerType,
        trigger_at: Option<DateTime<Utc>>,
        trigger_event: Option<&str>,
        trigger_filter: Option<&serde_json::Value>,
        ttl_secs: i64,
        session_id: Option<&str>,
    ) -> Result<i64> {
        self.create_with_scope(
            owner_id,
            target_id,
            message,
            trigger_type,
            trigger_at,
            trigger_event,
            trigger_filter,
            ttl_secs,
            session_id,
            None,
            false,
        )
    }

    fn create_with_scope(
        &self,
        owner_id: &str,
        target_id: Option<&str>,
        message: &str,
        trigger_type: ReminderTriggerType,
        trigger_at: Option<DateTime<Utc>>,
        trigger_event: Option<&str>,
        trigger_filter: Option<&serde_json::Value>,
        ttl_secs: i64,
        session_id: Option<&str>,
        origin_session_id: Option<&str>,
        cross_session: bool,
    ) -> Result<i64> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let trigger_at_str = trigger_at.map(|dt| dt.to_rfc3339());
        let trigger_filter_str = trigger_filter.map(|f| f.to_string());
        let resolved_target = target_id.unwrap_or(owner_id);
        let session_id = session_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        let origin_session_id = origin_session_id
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string);

        conn.execute(
            "INSERT INTO reminders (supervisor_id, message, trigger_type, trigger_at, trigger_event, trigger_filter, ttl_secs, created_at, target_id, session_id, origin_session_id, cross_session)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            params![
                owner_id,
                message,
                trigger_type.to_string(),
                trigger_at_str,
                trigger_event,
                trigger_filter_str,
                ttl_secs,
                now,
                resolved_target,
                session_id,
                origin_session_id,
                cross_session,
            ],
        )?;

        Ok(conn.last_insert_rowid())
    }

    fn list_pending(&self, owner_id: &str) -> Result<Vec<Reminder>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {} FROM reminders WHERE supervisor_id = ? AND status = 'pending' ORDER BY created_at ASC",
            Self::SELECT_COLUMNS
        );

        let mut stmt = conn.prepare_cached(&sql)?;
        let reminders = stmt
            .query_map(params![owner_id], Self::reminder_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(reminders)
    }

    fn list_pending_for_target(&self, target_id: &str) -> Result<Vec<Reminder>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {} FROM reminders WHERE target_id = ? AND status = 'pending' ORDER BY created_at ASC",
            Self::SELECT_COLUMNS
        );

        let mut stmt = conn.prepare_cached(&sql)?;
        let reminders = stmt
            .query_map(params![target_id], Self::reminder_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(reminders)
    }

    fn list_all_pending(&self) -> Result<Vec<Reminder>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {} FROM reminders WHERE status = 'pending' ORDER BY created_at ASC",
            Self::SELECT_COLUMNS
        );

        let mut stmt = conn.prepare_cached(&sql)?;
        let reminders = stmt
            .query_map([], Self::reminder_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(reminders)
    }

    fn list_recently_fired(&self, within_secs: i64) -> Result<Vec<Reminder>> {
        let conn = self.conn.lock().unwrap();
        let cutoff = (Utc::now() - chrono::Duration::seconds(within_secs)).to_rfc3339();
        let sql = format!(
            "SELECT {} FROM reminders WHERE status = 'fired' AND fired_at >= ? ORDER BY fired_at DESC",
            Self::SELECT_COLUMNS
        );

        let mut stmt = conn.prepare_cached(&sql)?;
        let reminders = stmt
            .query_map(params![cutoff], Self::reminder_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(reminders)
    }

    fn get_due_time_reminders(&self) -> Result<Vec<Reminder>> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let sql = format!(
            "SELECT {} FROM reminders WHERE trigger_type = 'time' AND status = 'pending' AND trigger_at <= ? ORDER BY trigger_at ASC",
            Self::SELECT_COLUMNS
        );

        let mut stmt = conn.prepare_cached(&sql)?;
        let reminders = stmt
            .query_map(params![now], Self::reminder_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(reminders)
    }

    fn get_event_reminders(&self, event_type: &str) -> Result<Vec<Reminder>> {
        let conn = self.conn.lock().unwrap();
        let sql = format!(
            "SELECT {} FROM reminders WHERE trigger_type = 'event' AND status = 'pending' AND trigger_event = ? ORDER BY created_at ASC",
            Self::SELECT_COLUMNS
        );

        let mut stmt = conn.prepare_cached(&sql)?;
        let reminders = stmt
            .query_map(params![event_type], Self::reminder_from_row)?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(reminders)
    }

    fn mark_fired(&self, id: i64, fired_event: Option<&serde_json::Value>) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let event_str = fired_event.map(|e| e.to_string());

        let rows = conn.execute(
            "UPDATE reminders SET status = 'fired', fired_at = ?, fired_event = ? WHERE id = ? AND status = 'pending'",
            params![now, event_str, id],
        )?;

        if rows == 0 {
            return Err(StoreError::NotFound(format!(
                "Reminder not found or not pending: {id}"
            )));
        }

        Ok(())
    }

    fn cancel(&self, id: i64, owner_id: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();

        let rows = conn.execute(
            "UPDATE reminders SET status = 'cancelled', cancelled_at = ? WHERE id = ? AND supervisor_id = ? AND status = 'pending'",
            params![now, id, owner_id],
        )?;

        if rows == 0 {
            return Err(StoreError::NotFound(format!(
                "Reminder not found, not pending, or not owned by this agent: {id}"
            )));
        }

        Ok(())
    }

    fn cancel_pending_for_target(&self, target_id: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let rows = conn.execute(
            "UPDATE reminders SET status = 'cancelled', cancelled_at = ?1 \
             WHERE status = 'pending' AND (target_id = ?2 OR (target_id IS NULL AND supervisor_id = ?2))",
            params![now, target_id],
        )?;
        Ok(rows)
    }

    fn cancel_pending_for_origin_session(&self, origin_session_id: &str) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let now = Utc::now().to_rfc3339();
        let rows = conn.execute(
            "UPDATE reminders SET status = 'cancelled', cancelled_at = ?1 \
             WHERE status = 'pending' AND origin_session_id = ?2 \
             AND COALESCE(cross_session, 0) = 0",
            params![now, origin_session_id],
        )?;
        Ok(rows)
    }

    fn expire_stale(&self) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        expire_stale_with_conn(&conn)
    }

    fn prune(&self, older_than_days: i64) -> Result<usize> {
        let conn = self.conn.lock().unwrap();
        let cutoff = (Utc::now() - chrono::Duration::days(older_than_days)).to_rfc3339();

        let rows = conn.execute(
            "DELETE FROM reminders WHERE status != 'pending' AND created_at < ?",
            params![cutoff],
        )?;

        Ok(rows)
    }

    fn close(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn create_test_store() -> (TempDir, SqliteReminderStore) {
        let temp = TempDir::new().unwrap();
        let store = SqliteReminderStore::open(temp.path()).unwrap();
        store.init().unwrap();
        (temp, store)
    }

    #[test]
    fn test_create_time_reminder() {
        let (_temp, store) = create_test_store();

        let fire_at = Utc::now() + chrono::Duration::seconds(300);
        let id = store
            .create(
                "supervisor-1",
                None,
                "Check worker-3 progress",
                ReminderTriggerType::Time,
                Some(fire_at),
                None,
                None,
                3600,
                None, // session_id cas-fcd4
            )
            .unwrap();

        assert!(id > 0);

        let pending = store.list_pending("supervisor-1").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].message, "Check worker-3 progress");
        assert_eq!(pending[0].trigger_type, ReminderTriggerType::Time);
        assert!(pending[0].trigger_at.is_some());
        // Self-reminder: target defaults to owner
        assert_eq!(pending[0].owner_id, "supervisor-1");
        assert_eq!(pending[0].target_id, "supervisor-1");
    }

    /// cas-f08d (GH #147): the delivery wire format is a contract between the
    /// daemon that writes it and `worker_status`, which reads it to tell a
    /// reminder a worker already acted on from mail it never woke for. Writing
    /// and parsing must agree for every shape the daemon can produce.
    #[test]
    fn reminder_delivery_format_and_parse_round_trip() {
        let timed = format_reminder_delivery(44, "check CI again", None);
        assert_eq!(timed, "Reminder #44: check CI again");
        assert_eq!(parse_reminder_delivery_id(&timed), Some(44));

        let evented = format_reminder_delivery(7, "review the merge", Some("task cas-1 completed"));
        assert_eq!(
            evented,
            "Reminder #7: review the merge (triggered by: task cas-1 completed)"
        );
        assert_eq!(parse_reminder_delivery_id(&evented), Some(7));
    }

    #[test]
    fn cross_session_delivery_includes_origin_and_created_at() {
        let (_temp, store) = create_test_store();
        let id = store
            .create_with_scope(
                "supervisor-1",
                None,
                "check the landing train",
                ReminderTriggerType::Time,
                Some(Utc::now() + chrono::Duration::minutes(5)),
                None,
                None,
                3600,
                Some("factory-a"),
                Some("origin-session"),
                true,
            )
            .unwrap();
        let reminder = store.list_pending("supervisor-1").unwrap().pop().unwrap();
        assert_eq!(reminder.id, id);
        let delivery = format_cross_session_reminder_delivery(&reminder, None);
        assert!(delivery.contains("Cross-session reminder"));
        assert!(delivery.contains("Origin session: origin-session"));
        assert!(delivery.contains("Created at:"));
        assert_eq!(parse_reminder_delivery_id(&delivery), Some(id));
    }

    #[test]
    fn cancelling_creator_session_keeps_only_cross_session_opt_in() {
        let (_temp, store) = create_test_store();
        for cross_session in [false, true] {
            store
                .create_with_scope(
                    "supervisor-1",
                    None,
                    "session boundary test",
                    ReminderTriggerType::Time,
                    Some(Utc::now() + chrono::Duration::minutes(5)),
                    None,
                    None,
                    3600,
                    Some("factory-a"),
                    Some("origin-session"),
                    cross_session,
                )
                .unwrap();
        }

        assert_eq!(
            store
                .cancel_pending_for_origin_session("origin-session")
                .unwrap(),
            1
        );
        let pending = store.list_pending("supervisor-1").unwrap();
        assert_eq!(pending.len(), 1);
        assert!(pending[0].cross_session);
        assert_eq!(
            pending[0].origin_session_id.as_deref(),
            Some("origin-session")
        );
    }

    /// Ordinary mail must never parse as a delivery: mistaking a supervisor's
    /// message for a reminder the worker already handled would discount real
    /// unread work and silence a genuine wedge.
    #[test]
    fn ordinary_mail_is_not_parsed_as_a_reminder_delivery() {
        assert_eq!(
            parse_reminder_delivery_id("Reminder: stop polling and close the task"),
            None
        );
        assert_eq!(parse_reminder_delivery_id("Reminder #abc: malformed"), None);
        assert_eq!(
            parse_reminder_delivery_id("Reminder #12 without a colon"),
            None
        );
        assert_eq!(
            parse_reminder_delivery_id("Please set a Reminder #44: for the CI run"),
            None
        );
    }

    #[test]
    fn test_create_targeted_reminder() {
        let (_temp, store) = create_test_store();

        let fire_at = Utc::now() + chrono::Duration::seconds(60);
        let id = store
            .create(
                "supervisor-1",
                Some("worker-fox"),
                "Sync your branch",
                ReminderTriggerType::Time,
                Some(fire_at),
                None,
                None,
                3600,
                None, // session_id cas-fcd4
            )
            .unwrap();

        assert!(id > 0);

        let pending = store.list_pending("supervisor-1").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].owner_id, "supervisor-1");
        assert_eq!(pending[0].target_id, "worker-fox");
        assert_eq!(pending[0].message, "Sync your branch");
    }

    #[test]
    fn test_create_event_reminder() {
        let (_temp, store) = create_test_store();

        let filter = serde_json::json!({"task_id": "cas-a1b2"});
        let id = store
            .create(
                "supervisor-1",
                None,
                "Review task output",
                ReminderTriggerType::Event,
                None,
                Some("task_completed"),
                Some(&filter),
                3600,
                None, // session_id cas-fcd4
            )
            .unwrap();

        assert!(id > 0);

        let pending = store.list_pending("supervisor-1").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].trigger_event.as_deref(), Some("task_completed"));
        assert_eq!(
            pending[0].trigger_filter,
            Some(serde_json::json!({"task_id": "cas-a1b2"}))
        );
    }

    #[test]
    fn test_get_due_time_reminders() {
        let (_temp, store) = create_test_store();

        // Create a reminder that is already due (fire_at in the past)
        let past = Utc::now() - chrono::Duration::seconds(10);
        store
            .create(
                "supervisor-1",
                None,
                "Past reminder",
                ReminderTriggerType::Time,
                Some(past),
                None,
                None,
                3600,
                None, // session_id cas-fcd4
            )
            .unwrap();

        // Create a reminder that is not yet due (fire_at in the future)
        let future = Utc::now() + chrono::Duration::seconds(300);
        store
            .create(
                "supervisor-1",
                None,
                "Future reminder",
                ReminderTriggerType::Time,
                Some(future),
                None,
                None,
                3600,
                None, // session_id cas-fcd4
            )
            .unwrap();

        let due = store.get_due_time_reminders().unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].message, "Past reminder");
    }

    #[test]
    fn test_get_event_reminders() {
        let (_temp, store) = create_test_store();

        store
            .create(
                "supervisor-1",
                None,
                "On task complete",
                ReminderTriggerType::Event,
                None,
                Some("task_completed"),
                None,
                3600,
                None, // session_id cas-fcd4
            )
            .unwrap();

        store
            .create(
                "supervisor-1",
                None,
                "On worker idle",
                ReminderTriggerType::Event,
                None,
                Some("worker_idle"),
                None,
                3600,
                None, // session_id cas-fcd4
            )
            .unwrap();

        let reminders = store.get_event_reminders("task_completed").unwrap();
        assert_eq!(reminders.len(), 1);
        assert_eq!(reminders[0].message, "On task complete");

        let reminders = store.get_event_reminders("worker_idle").unwrap();
        assert_eq!(reminders.len(), 1);
        assert_eq!(reminders[0].message, "On worker idle");

        let reminders = store.get_event_reminders("epic_completed").unwrap();
        assert!(reminders.is_empty());
    }

    #[test]
    fn test_mark_fired() {
        let (_temp, store) = create_test_store();

        let past = Utc::now() - chrono::Duration::seconds(10);
        let id = store
            .create(
                "supervisor-1",
                None,
                "Fire me",
                ReminderTriggerType::Time,
                Some(past),
                None,
                None,
                3600,
                None, // session_id cas-fcd4
            )
            .unwrap();

        let event = serde_json::json!({
            "event_type": "task_completed",
            "data": {"task_id": "cas-123", "worker": "swift-fox"},
            "description": "swift-fox completed task cas-123",
        });
        store.mark_fired(id, Some(&event)).unwrap();

        // Should no longer appear in pending
        let pending = store.list_pending("supervisor-1").unwrap();
        assert!(pending.is_empty());

        // Should no longer appear in due
        let due = store.get_due_time_reminders().unwrap();
        assert!(due.is_empty());

        // Fired event should be persisted in recently fired list
        let fired = store.list_recently_fired(60).unwrap();
        assert_eq!(fired.len(), 1);
        assert_eq!(
            fired[0].fired_event.as_ref().unwrap()["event_type"],
            "task_completed"
        );

        // Double-fire should fail
        assert!(store.mark_fired(id, None).is_err());
    }

    #[test]
    fn test_cancel() {
        let (_temp, store) = create_test_store();

        let id = store
            .create(
                "supervisor-1",
                None,
                "Cancel me",
                ReminderTriggerType::Event,
                None,
                Some("task_completed"),
                None,
                3600,
                None, // session_id cas-fcd4
            )
            .unwrap();

        // Cancel by wrong owner should fail
        assert!(store.cancel(id, "supervisor-2").is_err());

        // Cancel by correct owner should succeed
        store.cancel(id, "supervisor-1").unwrap();

        let pending = store.list_pending("supervisor-1").unwrap();
        assert!(pending.is_empty());

        // Double-cancel should fail
        assert!(store.cancel(id, "supervisor-1").is_err());
    }

    #[test]
    fn test_expire_stale() {
        let (_temp, store) = create_test_store();

        // Create with very short TTL (1 second)
        let future = Utc::now() + chrono::Duration::seconds(9999);
        store
            .create(
                "supervisor-1",
                None,
                "Will expire",
                ReminderTriggerType::Time,
                Some(future),
                None,
                None,
                1,    // 1 second TTL
                None, // session_id cas-fcd4
            )
            .unwrap();

        // Create with long TTL
        store
            .create(
                "supervisor-1",
                None,
                "Will not expire",
                ReminderTriggerType::Time,
                Some(future),
                None,
                None,
                99999,
                None, // session_id cas-fcd4
            )
            .unwrap();

        // Sleep briefly so the short-TTL reminder expires
        std::thread::sleep(std::time::Duration::from_secs(2));

        let expired = store.expire_stale().unwrap();
        assert_eq!(expired, 1);

        let pending = store.list_pending("supervisor-1").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].message, "Will not expire");
    }

    #[test]
    fn bounded_expiry_defers_busy_within_budget_and_restores_default_timeout() {
        let (temp, store) = create_test_store();

        let local_guard = store.conn.lock().unwrap();
        let local_started = std::time::Instant::now();
        assert_eq!(
            expire_stale_bounded(temp.path(), Duration::from_millis(50)).unwrap(),
            ReminderExpiryOutcome::DeferredBusy
        );
        // GH #92: this bound only discriminates "deferred promptly" from
        // "inherited the shared 5s SQLITE_BUSY_TIMEOUT". Keep it far below 5s
        // but wide enough that a scheduler stall on a saturated test host
        // cannot fail a correct run.
        assert!(
            local_started.elapsed() < Duration::from_millis(2000),
            "in-process connection contention must defer without waiting"
        );
        drop(local_guard);

        let blocker = Connection::open(temp.path().join("cas.db")).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();

        let busy_budget = Duration::from_millis(50);
        let started = std::time::Instant::now();
        let outcome = expire_stale_bounded(temp.path(), busy_budget).unwrap();
        let elapsed = started.elapsed();

        assert_eq!(outcome, ReminderExpiryOutcome::DeferredBusy);
        // GH #92: same discrimination as above — 50ms budget vs the 5s shared
        // timeout. 2s tolerates load-induced stalls while still proving the
        // budget was not inherited.
        assert!(
            elapsed < Duration::from_millis(2000),
            "50ms SQLite budget must not inherit the shared 5s timeout; elapsed={elapsed:?}"
        );

        blocker.execute_batch("ROLLBACK").unwrap();
        assert!(matches!(
            expire_stale_bounded(temp.path(), busy_budget).unwrap(),
            ReminderExpiryOutcome::Expired(_)
        ));

        let restored_timeout_ms: i64 = store
            .conn
            .lock()
            .unwrap()
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            restored_timeout_ms,
            crate::SQLITE_BUSY_TIMEOUT.as_millis() as i64
        );
    }

    #[test]
    fn test_owner_scoping() {
        let (_temp, store) = create_test_store();

        store
            .create(
                "supervisor-1",
                None,
                "For sup 1",
                ReminderTriggerType::Event,
                None,
                Some("task_completed"),
                None,
                3600,
                None, // session_id cas-fcd4
            )
            .unwrap();

        store
            .create(
                "supervisor-2",
                None,
                "For sup 2",
                ReminderTriggerType::Event,
                None,
                Some("task_completed"),
                None,
                3600,
                None, // session_id cas-fcd4
            )
            .unwrap();

        let pending1 = store.list_pending("supervisor-1").unwrap();
        assert_eq!(pending1.len(), 1);
        assert_eq!(pending1[0].message, "For sup 1");

        let pending2 = store.list_pending("supervisor-2").unwrap();
        assert_eq!(pending2.len(), 1);
        assert_eq!(pending2[0].message, "For sup 2");

        // get_event_reminders returns all owners (daemon processes all)
        let all = store.get_event_reminders("task_completed").unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_session_id_persisted_on_create() {
        let (_temp, store) = create_test_store();
        let id = store
            .create(
                "sup-a",
                None,
                "session scoped",
                ReminderTriggerType::Event,
                None,
                Some("task_completed"),
                None,
                3600,
                Some("factory-session-a"),
            )
            .unwrap();
        assert!(id > 0);
        let pending = store.list_pending("sup-a").unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].session_id.as_deref(), Some("factory-session-a"));
    }

    #[test]
    fn test_session_id_empty_stored_as_none() {
        let (_temp, store) = create_test_store();
        store
            .create(
                "sup-a",
                None,
                "blank session",
                ReminderTriggerType::Event,
                None,
                Some("task_completed"),
                None,
                3600,
                Some("   "),
            )
            .unwrap();
        let pending = store.list_pending("sup-a").unwrap();
        assert_eq!(pending[0].session_id, None);
    }

    #[test]
    fn test_event_reminders_from_two_sessions_both_listed() {
        // Store returns both; session filter is applied at fire time (daemon).
        let (_temp, store) = create_test_store();
        store
            .create(
                "sup-a",
                None,
                "from A",
                ReminderTriggerType::Event,
                None,
                Some("task_completed"),
                None,
                3600,
                Some("session-a"),
            )
            .unwrap();
        store
            .create(
                "sup-b",
                None,
                "from B",
                ReminderTriggerType::Event,
                None,
                Some("task_completed"),
                None,
                3600,
                Some("session-b"),
            )
            .unwrap();
        let all = store.get_event_reminders("task_completed").unwrap();
        assert_eq!(all.len(), 2);
        let sessions: Vec<_> = all.iter().filter_map(|r| r.session_id.as_deref()).collect();
        assert!(sessions.contains(&"session-a"));
        assert!(sessions.contains(&"session-b"));
    }
}
