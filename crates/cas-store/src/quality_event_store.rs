//! Friction ledger: the append-only `quality_events` table.
//!
//! Every check, supervisor intervention, escape, release and feature verdict
//! lands here automatically from the code path that observed it; nobody types
//! into it. Triggers refuse UPDATE and DELETE, so `seq` is a stable push cursor
//! and a row never changes after it has been reported.
//!
//! Cloud data scope A applies at this boundary. A payload is typed metrics and
//! enums, `title` and `link` are sanitized on write, and `local_note` (reasons,
//! free text) is excluded from [`QualityEvent::to_wire`]. No code or snippets
//! can leave a project through this table.

use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, Row, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::Result;
use crate::error::StoreError;

/// DDL shared by store-open repair and the numbered migration.
pub const QUALITY_EVENT_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS quality_events (
        seq INTEGER PRIMARY KEY AUTOINCREMENT,
        id TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN
            ('check_run', 'intervention', 'escape', 'release', 'feature_feedback', 'turn')),
        occurred_at TEXT NOT NULL,
        task_id TEXT,
        release_version TEXT,
        actor TEXT,
        title TEXT,
        link TEXT,
        payload TEXT NOT NULL,
        local_note TEXT,
        origin TEXT NOT NULL DEFAULT 'local' CHECK (origin IN ('local', 'cloud')),
        project_id TEXT NOT NULL DEFAULT '',
        UNIQUE (project_id, id)
    )",
    "CREATE INDEX IF NOT EXISTS idx_quality_events_occurred ON quality_events(occurred_at)",
    "CREATE INDEX IF NOT EXISTS idx_quality_events_task ON quality_events(task_id, seq)",
    "CREATE TRIGGER IF NOT EXISTS quality_events_no_update BEFORE UPDATE ON quality_events
     BEGIN SELECT RAISE(ABORT, 'quality_events is append-only'); END",
    "CREATE TRIGGER IF NOT EXISTS quality_events_no_delete BEFORE DELETE ON quality_events
     BEGIN SELECT RAISE(ABORT, 'quality_events is append-only'); END",
];

/// Rebuild the historical CHECK without changing any row. Caller owns the transaction.
pub const QUALITY_EVENT_TURN_UPGRADE_STATEMENTS: &[&str] = &[
    "CREATE TABLE quality_events_turn_upgrade (
        seq INTEGER PRIMARY KEY AUTOINCREMENT, id TEXT NOT NULL,
        kind TEXT NOT NULL CHECK (kind IN
            ('check_run', 'intervention', 'escape', 'release', 'feature_feedback', 'turn')),
        occurred_at TEXT NOT NULL, task_id TEXT, release_version TEXT, actor TEXT,
        title TEXT, link TEXT, payload TEXT NOT NULL, local_note TEXT,
        origin TEXT NOT NULL DEFAULT 'local' CHECK (origin IN ('local', 'cloud')),
        project_id TEXT NOT NULL DEFAULT '', UNIQUE (project_id, id)
    )",
    "INSERT INTO quality_events_turn_upgrade SELECT * FROM quality_events",
    "DROP TABLE quality_events",
    "ALTER TABLE quality_events_turn_upgrade RENAME TO quality_events",
    QUALITY_EVENT_SCHEMA_STATEMENTS[1],
    QUALITY_EVENT_SCHEMA_STATEMENTS[2],
    QUALITY_EVENT_SCHEMA_STATEMENTS[3],
    QUALITY_EVENT_SCHEMA_STATEMENTS[4],
];

/// Immutable channel evidence; never inferred from sender prose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WakeChannel { Operator, Terminal, Director, Supervisor, Worker, Daemon, Unknown }

impl WakeChannel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Operator => "operator", Self::Terminal => "terminal",
            Self::Director => "director", Self::Supervisor => "supervisor",
            Self::Worker => "worker", Self::Daemon => "daemon", Self::Unknown => "unknown",
        }
    }
}

/// Observed successful actions only; no arguments or text cross this boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TurnAction { TaskMutation, FileEdit, Commit, Push, Check, Decision, Assignment, Answer }

/// Largest batch a single Cloud request may carry (client contract).
pub const QUALITY_EVENT_BATCH_LIMIT: usize = 200;

// Cloud caps: title 200, link 2048, payload 4 KB. Tighter here, so a size
// refusal never reaches `rejected[]`.
const TITLE_MAX_CHARS: usize = 120;
const LINK_MAX_CHARS: usize = 300;
const PAYLOAD_MAX_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStage {
    Lane,
    Queue,
    Verifier,
    Qa,
    Journey,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckOutcome {
    Pass,
    Fail,
    Flaky,
    /// The check itself could not finish, so it says nothing about the delivery.
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InterventionKind {
    HandMerge,
    OverrideClose,
    OverrideMerge,
    ForceShutdown,
    Respawn,
    Rebind,
    StaleAlert,
}

impl InterventionKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::HandMerge => "hand_merge",
            Self::OverrideClose => "override_close",
            Self::OverrideMerge => "override_merge",
            Self::ForceShutdown => "force_shutdown",
            Self::Respawn => "respawn",
            Self::Rebind => "rebind",
            Self::StaleAlert => "stale_alert",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackVerdict {
    Helped,
    Noise,
    Harmful,
    Unused,
}

impl FeedbackVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Helped => "helped",
            Self::Noise => "noise",
            Self::Harmful => "harmful",
            Self::Unused => "unused",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "helped" => Some(Self::Helped),
            "noise" => Some(Self::Noise),
            "harmful" => Some(Self::Harmful),
            "unused" => Some(Self::Unused),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeedbackSource {
    Measured,
    Supervisor,
}

/// Kind-specific metrics. Strings are slugs, versions or hex; never free text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QualityPayload {
    Turn {
        woken_by: WakeChannel,
        actions: Vec<TurnAction>,
        no_op: bool,
        input_tokens: Option<u64>,
        output_tokens: Option<u64>,
        minutes: Option<f64>,
    },
    CheckRun {
        check_id: String,
        stage: CheckStage,
        outcome: CheckOutcome,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sha: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        minutes: Option<f64>,
    },
    Intervention {
        intervention: InterventionKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        minutes: Option<f64>,
    },
    Escape {
        found_in: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        missed_stage: Option<CheckStage>,
    },
    Release {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        commit_to_main_minutes: Option<f64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        main_to_published_minutes: Option<f64>,
        #[serde(default)]
        ejections: u32,
        #[serde(default)]
        rescues: u32,
    },
    FeatureFeedback {
        feature: String,
        verdict: FeedbackVerdict,
        source: FeedbackSource,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        injected: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        opened: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        used: Option<u64>,
    },
}

impl QualityPayload {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Turn { .. } => "turn",
            Self::CheckRun { .. } => "check_run",
            Self::Intervention { .. } => "intervention",
            Self::Escape { .. } => "escape",
            Self::Release { .. } => "release",
            Self::FeatureFeedback { .. } => "feature_feedback",
        }
    }

    fn validate(&self) -> std::result::Result<(), String> {
        match self {
            Self::Turn { actions, no_op, minutes, .. } => {
                if *no_op != actions.is_empty() || actions.len() > 8
                    || actions.iter().collect::<std::collections::BTreeSet<_>>().len() != actions.len()
                    || minutes.is_some_and(|n| !n.is_finite() || n < 0.0) {
                    return Err("inconsistent turn metrics".into());
                }
            }
            Self::CheckRun { check_id, sha, .. } => {
                if !is_slug(check_id) {
                    return Err(format!("check_id is not a slug: {check_id}"));
                }
                if let Some(sha) = sha
                    && !(sha.len() >= 7 && sha.len() <= 64 && sha.bytes().all(|b| b.is_ascii_hexdigit()))
                {
                    return Err(format!("sha is not hex: {sha}"));
                }
            }
            Self::Escape { found_in, .. } if !is_version(found_in) => {
                return Err(format!("found_in is not a version: {found_in}"));
            }
            Self::FeatureFeedback { feature, .. } if !is_slug(feature) => {
                return Err(format!("feature is not a slug: {feature}"));
            }
            _ => {}
        }
        Ok(())
    }
}

/// What an emitter observed. `natural_key` identifies the fact, so a retry or a
/// second machine reporting the same fact collapses to one row. `scope` is the
/// project: Cloud is idempotent per team, and short task ids repeat across a
/// team's projects, so an id without it could silently drop another project's row.
#[derive(Debug, Clone)]
pub struct NewQualityEvent {
    pub scope: String,
    pub natural_key: String,
    pub occurred_at: DateTime<Utc>,
    pub task_id: Option<String>,
    pub release_version: Option<String>,
    pub actor: Option<String>,
    pub title: Option<String>,
    pub link: Option<String>,
    pub payload: QualityPayload,
    pub local_note: Option<String>,
}

impl NewQualityEvent {
    pub fn new(natural_key: impl Into<String>, payload: QualityPayload) -> Self {
        Self {
            scope: String::new(),
            natural_key: natural_key.into(),
            occurred_at: Utc::now(),
            task_id: None,
            release_version: None,
            actor: None,
            title: None,
            link: None,
            payload,
            local_note: None,
        }
    }
}

/// A stored row. `project_id` is empty for this project's own events.
#[derive(Debug, Clone, PartialEq)]
pub struct QualityEvent {
    pub seq: i64,
    pub id: String,
    pub occurred_at: DateTime<Utc>,
    pub task_id: Option<String>,
    pub release_version: Option<String>,
    pub actor: Option<String>,
    pub title: Option<String>,
    pub link: Option<String>,
    pub payload: QualityPayload,
    pub local_note: Option<String>,
    pub from_cloud: bool,
    pub project_id: String,
}

/// One event on the Cloud wire: scope A only. `local_note` has no field here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QualityEventWire {
    pub id: String,
    /// Mirrors the payload tag so the server can validate without parsing it.
    #[serde(default)]
    pub kind: String,
    pub occurred_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub actor: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
    pub payload: QualityPayload,
    /// Set per event on push by the syncer; returned by the server on pull.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

impl QualityEvent {
    pub fn to_wire(&self) -> QualityEventWire {
        QualityEventWire {
            id: self.id.clone(),
            kind: self.payload.kind().to_string(),
            occurred_at: self.occurred_at,
            task_id: self.task_id.clone(),
            release_version: self.release_version.clone(),
            actor: self.actor.clone(),
            title: self.title.clone(),
            link: self.link.clone(),
            payload: self.payload.clone(),
            project_id: None,
        }
    }
}

fn is_slug(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 96
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':' | b'/'))
}

fn is_version(value: &str) -> bool {
    let value = value.strip_prefix('v').unwrap_or(value);
    let parts: Vec<_> = value.split('.').collect();
    parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

fn is_identifier(value: &str) -> bool {
    !value.is_empty() && value.len() <= 96 && !value.chars().any(char::is_whitespace)
}

/// Scope-A title: one short line with inline code, paths and KEY=VALUE removed.
pub fn sanitize_quality_title(raw: &str) -> Option<String> {
    let line = raw.lines().next().unwrap_or_default();
    // Backtick spans are the way titles quote code or commands.
    let mut without_code = String::with_capacity(line.len());
    for (index, span) in line.split('`').enumerate() {
        if index % 2 == 0 {
            without_code.push_str(span);
            without_code.push(' ');
        }
    }
    let kept: Vec<&str> = without_code
        .split_whitespace()
        .filter(|word| {
            let bare = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '/' && c != '~');
            !(bare.starts_with('/')
                || bare.starts_with("~/")
                || bare.contains("://")
                || bare.contains('='))
        })
        .collect();
    let joined = kept.join(" ");
    let title: String = joined.chars().take(TITLE_MAX_CHARS).collect();
    let title = title.trim().trim_end_matches([':', ',', ';', '-']).trim_end();
    (!title.is_empty()).then(|| title.to_string())
}

/// Scope-A link: an https URL with no query or fragment, which can carry tokens.
pub fn sanitize_quality_link(raw: &str) -> Option<String> {
    let link = raw.trim();
    (link.starts_with("https://")
        && link.chars().count() <= LINK_MAX_CHARS
        && !link.contains(['?', '#'])
        && !link.chars().any(char::is_whitespace))
    .then(|| link.to_string())
}

/// `vX.Y.Z` and `X.Y.Z` name one release.
pub fn normalize_release_version(raw: &str) -> Option<String> {
    let raw = raw.trim();
    is_version(raw).then(|| format!("v{}", raw.strip_prefix('v').unwrap_or(raw)))
}

fn event_id(scope: &str, kind: &str, natural_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(scope.as_bytes());
    hasher.update([0]);
    hasher.update(kind.as_bytes());
    hasher.update([0]);
    hasher.update(natural_key.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest[..16].iter().map(|byte| format!("{byte:02x}")).collect();
    format!("qe-{hex}")
}

fn format_time(time: DateTime<Utc>) -> String {
    time.to_rfc3339_opts(SecondsFormat::Secs, true)
}

fn invalid(message: String) -> StoreError {
    StoreError::Other(format!("invalid quality event: {message}"))
}

/// Create the table, indexes and append-only triggers when missing.
pub fn ensure_quality_event_schema(conn: &Connection) -> Result<()> {
    let historical = conn.query_row(
        "SELECT sql FROM sqlite_master WHERE type='table' AND name='quality_events'",
        [], |row| row.get::<_, String>(0),
    ).ok().is_some_and(|sql| !sql.contains("'turn'"));
    conn.execute_batch("SAVEPOINT quality_event_schema")?;
    let result = (|| -> Result<()> {
        let statements = if historical { QUALITY_EVENT_TURN_UPGRADE_STATEMENTS }
            else { QUALITY_EVENT_SCHEMA_STATEMENTS };
        for statement in statements { conn.execute(statement, [])?; }
        Ok(())
    })();
    if result.is_err() { let _ = conn.execute_batch("ROLLBACK TO quality_event_schema"); }
    conn.execute_batch("RELEASE quality_event_schema")?;
    result
}

fn with_conn<T>(cas_dir: &Path, work: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    let conn = crate::shared_db::shared_connection(&cas_dir.join("cas.db"))?;
    let guard = crate::shared_db::lock_connection(&conn)?;
    ensure_quality_event_schema(&guard)?;
    work(&guard)
}

/// Append one locally observed event. Returns false when the fact is already
/// recorded.
pub fn record_quality_event(cas_dir: &Path, event: &NewQualityEvent) -> Result<bool> {
    event.payload.validate().map_err(invalid)?;
    let task_id = event.task_id.as_deref().filter(|id| is_identifier(id));
    let release_version = event
        .release_version
        .as_deref()
        .and_then(normalize_release_version);
    let actor = event.actor.as_deref().filter(|actor| is_identifier(actor));
    let title = event.title.as_deref().and_then(sanitize_quality_title);
    let link = event.link.as_deref().and_then(sanitize_quality_link);
    let payload = serde_json::to_string(&event.payload).map_err(|e| invalid(e.to_string()))?;
    if payload.len() > PAYLOAD_MAX_BYTES {
        return Err(invalid(format!("payload is {} bytes", payload.len())));
    }
    let kind = event.payload.kind();
    with_conn(cas_dir, |conn| {
        let inserted = conn.execute(
            "INSERT OR IGNORE INTO quality_events
             (id, kind, occurred_at, task_id, release_version, actor, title, link, payload, local_note)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                event_id(&event.scope, kind, &event.natural_key),
                kind,
                format_time(event.occurred_at),
                task_id,
                release_version,
                actor,
                title,
                link,
                payload,
                event.local_note.as_deref().map(str::trim).filter(|note| !note.is_empty()),
            ],
        )?;
        Ok(inserted > 0)
    })
}

/// Whether this local fact already exists under its canonical or a legacy
/// natural key. Resolve identities with the writer's hash and project scope;
/// old append-only rows need not retain links or carry new payload fields.
pub fn quality_event_recorded(
    cas_dir: &Path,
    event: &NewQualityEvent,
    aliases: &[String],
) -> Result<bool> {
    with_conn(cas_dir, |conn| {
        let mut query = conn.prepare("SELECT EXISTS(SELECT 1 FROM quality_events WHERE project_id='' AND id=?1)")?;
        for key in std::iter::once(&event.natural_key).chain(aliases) {
            if query.query_row([event_id(&event.scope, event.payload.kind(), key)], |row| row.get::<_, bool>(0))? {
                return Ok(true);
            }
        }
        Ok(false)
    })
}

/// Apply rows pulled from Cloud. `current_project` maps this project's rows back
/// to the local namespace, so its own echo and a second machine's copy collapse.
/// Returns (inserted, refused). Cloud rows are untrusted input: each one passes
/// the validation and sanitizing of a local write, or is refused.
pub fn import_quality_events(
    cas_dir: &Path,
    rows: &[QualityEventWire],
    current_project: &str,
) -> Result<(usize, usize)> {
    with_conn(cas_dir, |conn| {
        let (mut inserted, mut refused) = (0, 0);
        for row in rows {
            let project_id = row.project_id.as_deref().unwrap_or_default().trim();
            if project_id.is_empty() || !is_identifier(&row.id) || row.payload.validate().is_err() {
                refused += 1;
                continue;
            }
            let project_id = if project_id == current_project { "" } else { project_id };
            let payload = serde_json::to_string(&row.payload).map_err(|e| invalid(e.to_string()))?;
            inserted += conn.execute(
                "INSERT OR IGNORE INTO quality_events
                 (id, kind, occurred_at, task_id, release_version, actor, title, link, payload,
                  origin, project_id)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'cloud', ?10)",
                params![
                    row.id,
                    row.payload.kind(),
                    format_time(row.occurred_at),
                    row.task_id.as_deref().filter(|id| is_identifier(id)),
                    row.release_version.as_deref().and_then(normalize_release_version),
                    row.actor.as_deref().filter(|actor| is_identifier(actor)),
                    row.title.as_deref().and_then(sanitize_quality_title),
                    row.link.as_deref().and_then(sanitize_quality_link),
                    payload,
                    project_id,
                ],
            )?;
        }
        Ok((inserted, refused))
    })
}

const COLUMNS: &str = "seq, id, occurred_at, task_id, release_version, actor, title, link,
    payload, local_note, origin, project_id";

/// Rows whose stored payload no longer parses are skipped by the callers below;
/// a newer client's kind must not break an older client's report.
fn parse_row(row: &Row) -> rusqlite::Result<Option<QualityEvent>> {
    let occurred_at: String = row.get(2)?;
    let payload: String = row.get(8)?;
    let origin: String = row.get(10)?;
    let (Ok(occurred_at), Ok(payload)) = (
        DateTime::parse_from_rfc3339(&occurred_at),
        serde_json::from_str::<QualityPayload>(&payload),
    ) else {
        return Ok(None);
    };
    Ok(Some(QualityEvent {
        seq: row.get(0)?,
        id: row.get(1)?,
        occurred_at: occurred_at.with_timezone(&Utc),
        task_id: row.get(3)?,
        release_version: row.get(4)?,
        actor: row.get(5)?,
        title: row.get(6)?,
        link: row.get(7)?,
        payload,
        local_note: row.get(9)?,
        from_cloud: origin == "cloud",
        project_id: row.get(11)?,
    }))
}

fn collect(
    conn: &Connection,
    sql: &str,
    args: &[&dyn rusqlite::ToSql],
) -> Result<Vec<QualityEvent>> {
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt.query_map(args, parse_row)?;
    let mut events = Vec::new();
    for row in rows {
        if let Some(event) = row? {
            events.push(event);
        }
    }
    Ok(events)
}

/// The next batch of this machine's own events after a push cursor.
pub fn quality_events_after(
    cas_dir: &Path,
    after_seq: i64,
    limit: usize,
) -> Result<Vec<QualityEvent>> {
    with_conn(cas_dir, |conn| {
        collect(
            conn,
            &format!(
                "SELECT {COLUMNS} FROM quality_events
                 WHERE origin = 'local' AND seq > ?1 ORDER BY seq LIMIT ?2"
            ),
            &[&after_seq, &(limit as i64)],
        )
    })
}

/// Every event in `(from, to]`, across all synced projects, oldest first.
pub fn quality_events_between(
    cas_dir: &Path,
    from: Option<DateTime<Utc>>,
    to: Option<DateTime<Utc>>,
) -> Result<Vec<QualityEvent>> {
    let from = from.map(format_time).unwrap_or_default();
    let to = to.map(format_time).unwrap_or_else(|| "9999".to_string());
    with_conn(cas_dir, |conn| {
        collect(
            conn,
            &format!(
                "SELECT {COLUMNS} FROM quality_events
                 WHERE occurred_at > ?1 AND occurred_at <= ?2 ORDER BY occurred_at, seq"
            ),
            &[&from, &to],
        )
    })
}

/// This project's release events, newest first.
pub fn quality_release_events(cas_dir: &Path) -> Result<Vec<QualityEvent>> {
    with_conn(cas_dir, |conn| {
        collect(
            conn,
            &format!(
                "SELECT {COLUMNS} FROM quality_events
                 WHERE kind = 'release' AND project_id = '' ORDER BY occurred_at DESC, seq DESC"
            ),
            &[],
        )
    })
}

/// This project's events for one task, oldest first.
pub fn quality_events_for_task(cas_dir: &Path, task_id: &str) -> Result<Vec<QualityEvent>> {
    with_conn(cas_dir, |conn| {
        collect(
            conn,
            &format!(
                "SELECT {COLUMNS} FROM quality_events
                 WHERE task_id = ?1 AND project_id = '' ORDER BY seq"
            ),
            &[&task_id],
        )
    })
}

/// This project's interventions of one kind since a time, newest first.
pub fn quality_interventions_since(
    cas_dir: &Path,
    kind: InterventionKind,
    since: DateTime<Utc>,
) -> Result<Vec<QualityEvent>> {
    let since = format_time(since);
    let events = with_conn(cas_dir, |conn| {
        collect(
            conn,
            &format!(
                "SELECT {COLUMNS} FROM quality_events
                 WHERE kind = 'intervention' AND project_id = '' AND occurred_at >= ?1
                 ORDER BY seq DESC"
            ),
            &[&since],
        )
    })?;
    Ok(events
        .into_iter()
        .filter(|event| {
            matches!(&event.payload, QualityPayload::Intervention { intervention, .. } if *intervention == kind)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn intervention(key: &str) -> NewQualityEvent {
        let mut event = NewQualityEvent::new(
            key,
            QualityPayload::Intervention {
                intervention: InterventionKind::Rebind,
                minutes: None,
            },
        );
        event.local_note = Some("reason with `rm -rf /secret`".into());
        event
    }

    #[test]
    fn ledger_is_append_only_and_refacts_collapse() {
        let dir = tempfile::tempdir().unwrap();
        assert!(record_quality_event(dir.path(), &intervention("a")).unwrap());
        assert!(!record_quality_event(dir.path(), &intervention("a")).unwrap());
        assert_eq!(quality_events_after(dir.path(), 0, 10).unwrap().len(), 1);
        assert!(!quality_event_recorded(dir.path(), &intervention("new-key"), &[]).unwrap());
        assert!(quality_event_recorded(dir.path(), &intervention("new-key"), &["a".into()]).unwrap());
        let mut other_scope = intervention("new-key");
        other_scope.scope = "other-project".into();
        assert!(!quality_event_recorded(dir.path(), &other_scope, &["a".into()]).unwrap());
        assert_eq!(quality_events_after(dir.path(), 0, 10).unwrap().len(), 1,
            "alias lookup must not append or modify the original event");

        let before = quality_events_after(dir.path(), 0, 10).unwrap();
        let conn = crate::shared_db::shared_connection(&dir.path().join("cas.db")).unwrap();
        let guard = crate::shared_db::lock_connection(&conn).unwrap();
        // Simulate a database shipped before Turn existed, preserving its rows.
        guard.execute_batch("DROP TRIGGER quality_events_no_update; DROP TRIGGER quality_events_no_delete; ALTER TABLE quality_events RENAME TO old_quality_events;").unwrap();
        let old_ddl = QUALITY_EVENT_SCHEMA_STATEMENTS[0].replace(", 'turn'", "");
        guard.execute(&old_ddl, []).unwrap();
        guard.execute("INSERT INTO quality_events SELECT * FROM old_quality_events", []).unwrap();
        guard.execute("DROP TABLE old_quality_events", []).unwrap();
        ensure_quality_event_schema(&guard).unwrap();
        ensure_quality_event_schema(&guard).unwrap();
        for statement in [
            "UPDATE quality_events SET title = 'x'",
            "DELETE FROM quality_events",
        ] {
            let error = guard.execute(statement, []).unwrap_err().to_string();
            assert!(error.contains("append-only"), "{statement}: {error}");
        }
        drop(guard);
        assert_eq!(quality_events_after(dir.path(), 0, 10).unwrap(), before);
        let mut turn = NewQualityEvent::new("turn", QualityPayload::Turn {
            woken_by: WakeChannel::Unknown, actions: vec![], no_op: true,
            input_tokens: None, output_tokens: None, minutes: Some(0.5),
        });
        assert!(record_quality_event(dir.path(), &turn).unwrap());
        let rows = quality_events_after(dir.path(), 0, 10).unwrap();
        assert!(rows[1].seq > before[0].seq);
        if let QualityPayload::Turn { no_op, .. } = &mut turn.payload { *no_op = false; }
        assert!(record_quality_event(dir.path(), &turn).is_err());
    }

    /// Scope A: code, command lines, paths, secrets and notes never reach the wire.
    #[test]
    fn scope_a_strips_titles_links_and_notes() {
        assert_eq!(
            sanitize_quality_title(
                "Fix `cargo run --token abc` in /home/me/.ssh/id KEY=secret flow\nfn main() {}"
            )
            .as_deref(),
            Some("Fix in flow"),
        );
        assert_eq!(
            sanitize_quality_title(&"long ".repeat(100)).unwrap().chars().count(),
            TITLE_MAX_CHARS - 1,
        );
        assert_eq!(sanitize_quality_link("https://x.test/run/1?token=abc"), None);
        assert_eq!(sanitize_quality_link("http://x.test/run/1"), None);

        let dir = tempfile::tempdir().unwrap();
        record_quality_event(dir.path(), &intervention("a")).unwrap();
        let stored = quality_events_after(dir.path(), 0, 10).unwrap().remove(0);
        assert!(stored.local_note.is_some());
        let wire = serde_json::to_string(&stored.to_wire()).unwrap();
        assert!(!wire.contains("secret"), "{wire}");

        let mut code = intervention("b");
        code.payload = QualityPayload::CheckRun {
            check_id: "fn main() { leak }".into(),
            stage: CheckStage::Lane,
            outcome: CheckOutcome::Fail,
            sha: None,
            minutes: None,
        };
        assert!(record_quality_event(dir.path(), &code).is_err());
    }
}
