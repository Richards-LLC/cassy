//! Epic splits between peer supervisors (cas-4736).
//!
//! Two supervisors of one repository, each with its own Cassy database,
//! agree who takes which children of an epic. Each database records the
//! split from its own side: `mine` are the children this side took, `theirs`
//! the children the peer took. A proposal starts `pending`; the peer's answer
//! moves it to `accepted` or `rejected`, and either side may later move it to
//! `cancelled`. Expiry of an unanswered proposal is computed from
//! `expires_at`, never stored.
//!
//! State changes are compare-and-set on the current state, so a concurrent
//! answer and cancel cannot both win.

use std::path::Path;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

use crate::Result;
use crate::error::StoreError;

/// DDL shared by store-open repair and the numbered migration.
pub const EPIC_SPLIT_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS epic_splits (
        id TEXT PRIMARY KEY,
        epic_id TEXT NOT NULL,
        local_agent_id TEXT NOT NULL,
        peer_agent_id TEXT NOT NULL,
        peer_name TEXT NOT NULL,
        peer_machine TEXT NOT NULL,
        direction TEXT NOT NULL CHECK (direction IN ('outgoing', 'incoming')),
        mine TEXT NOT NULL,
        theirs TEXT NOT NULL,
        state TEXT NOT NULL CHECK (state IN ('pending', 'accepted', 'rejected', 'cancelled')),
        reason TEXT,
        message_id TEXT,
        expires_at TEXT NOT NULL,
        created_at TEXT NOT NULL,
        decided_at TEXT
    )",
    "CREATE INDEX IF NOT EXISTS idx_epic_splits_state ON epic_splits(state, epic_id)",
];

const COLUMNS: &str = "id, epic_id, local_agent_id, peer_agent_id, peer_name, peer_machine,
    direction, mine, theirs, state, reason, message_id, expires_at, created_at, decided_at";

/// Which side proposed the split, seen from this database.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    /// This side proposed it.
    Outgoing,
    /// The peer proposed it.
    Incoming,
}

impl SplitDirection {
    pub fn as_str(self) -> &'static str {
        match self {
            SplitDirection::Outgoing => "outgoing",
            SplitDirection::Incoming => "incoming",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "outgoing" => Some(SplitDirection::Outgoing),
            "incoming" => Some(SplitDirection::Incoming),
            _ => None,
        }
    }
}

/// Stored state of a split.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitState {
    Pending,
    Accepted,
    Rejected,
    Cancelled,
}

impl SplitState {
    pub fn as_str(self) -> &'static str {
        match self {
            SplitState::Pending => "pending",
            SplitState::Accepted => "accepted",
            SplitState::Rejected => "rejected",
            SplitState::Cancelled => "cancelled",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(SplitState::Pending),
            "accepted" => Some(SplitState::Accepted),
            "rejected" => Some(SplitState::Rejected),
            "cancelled" => Some(SplitState::Cancelled),
            _ => None,
        }
    }
}

/// One split, from this database's side.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicSplit {
    /// Proposal id, shared by both sides.
    pub id: String,
    pub epic_id: String,
    /// The local supervisor that proposed or received it.
    pub local_agent_id: String,
    pub peer_agent_id: String,
    pub peer_name: String,
    pub peer_machine: String,
    pub direction: SplitDirection,
    /// Children this side takes.
    pub mine: Vec<String>,
    /// Children the peer takes.
    pub theirs: Vec<String>,
    pub state: SplitState,
    pub reason: Option<String>,
    /// Cloud peer message id of the proposal (incoming side).
    pub message_id: Option<String>,
    pub expires_at: DateTime<Utc>,
    pub created_at: DateTime<Utc>,
    pub decided_at: Option<DateTime<Utc>>,
}

impl EpicSplit {
    /// A pending proposal past its expiry.
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.state == SplitState::Pending && self.expires_at <= now
    }

    /// Whether the split currently binds this side: accepted, or this side's
    /// own unexpired pending proposal (it already claimed its share and
    /// offered the rest). An incoming proposal binds nothing until accepted.
    pub fn is_binding(&self, now: DateTime<Utc>) -> bool {
        match self.state {
            SplitState::Accepted => true,
            SplitState::Pending => {
                self.direction == SplitDirection::Outgoing && !self.is_expired(now)
            }
            SplitState::Rejected | SplitState::Cancelled => false,
        }
    }

    /// The state to show: `expired` for a lapsed pending proposal.
    pub fn display_state(&self, now: DateTime<Utc>) -> &'static str {
        if self.is_expired(now) {
            "expired"
        } else {
            self.state.as_str()
        }
    }
}

fn open_conn(cas_dir: &Path) -> Result<std::sync::Arc<std::sync::Mutex<Connection>>> {
    let conn = crate::shared_db::shared_connection(&cas_dir.join("cas.db"))?;
    {
        let guard = crate::shared_db::lock_connection(&conn)?;
        for statement in EPIC_SPLIT_SCHEMA_STATEMENTS {
            guard.execute(statement, [])?;
        }
    }
    Ok(conn)
}

fn parse_time(index: usize, value: String) -> rusqlite::Result<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(&value)
        .map(|time| time.with_timezone(&Utc))
        .map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                index,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })
}

fn bad_value(index: usize, value: &str) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        format!("unexpected value {value:?}").into(),
    )
}

fn parse_ids(index: usize, value: String) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, rusqlite::types::Type::Text, Box::new(error))
    })
}

fn parse_row(row: &rusqlite::Row) -> rusqlite::Result<EpicSplit> {
    let direction: String = row.get(6)?;
    let state: String = row.get(9)?;
    Ok(EpicSplit {
        id: row.get(0)?,
        epic_id: row.get(1)?,
        local_agent_id: row.get(2)?,
        peer_agent_id: row.get(3)?,
        peer_name: row.get(4)?,
        peer_machine: row.get(5)?,
        direction: SplitDirection::parse(&direction).ok_or_else(|| bad_value(6, &direction))?,
        mine: parse_ids(7, row.get(7)?)?,
        theirs: parse_ids(8, row.get(8)?)?,
        state: SplitState::parse(&state).ok_or_else(|| bad_value(9, &state))?,
        reason: row.get(10)?,
        message_id: row.get(11)?,
        expires_at: parse_time(12, row.get(12)?)?,
        created_at: parse_time(13, row.get(13)?)?,
        decided_at: row
            .get::<_, Option<String>>(14)?
            .map(|value| parse_time(14, value))
            .transpose()?,
    })
}

fn ids_json(ids: &[String]) -> Result<String> {
    serde_json::to_string(ids).map_err(|error| StoreError::Parse(error.to_string()))
}

/// Record a split. Idempotent on its id: returns `false` (and changes
/// nothing) when a split with this id is already recorded.
pub fn insert_epic_split(cas_dir: &Path, split: &EpicSplit) -> Result<bool> {
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    let inserted = conn.execute(
        &format!(
            "INSERT OR IGNORE INTO epic_splits ({COLUMNS})
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)"
        ),
        params![
            split.id,
            split.epic_id,
            split.local_agent_id,
            split.peer_agent_id,
            split.peer_name,
            split.peer_machine,
            split.direction.as_str(),
            ids_json(&split.mine)?,
            ids_json(&split.theirs)?,
            split.state.as_str(),
            split.reason,
            split.message_id,
            split.expires_at.to_rfc3339(),
            split.created_at.to_rfc3339(),
            split.decided_at.map(|at| at.to_rfc3339()),
        ],
    )?;
    Ok(inserted == 1)
}

/// One split by id.
pub fn get_epic_split(cas_dir: &Path, id: &str) -> Result<Option<EpicSplit>> {
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    Ok(conn
        .query_row(
            &format!("SELECT {COLUMNS} FROM epic_splits WHERE id = ?1"),
            params![id],
            parse_row,
        )
        .optional()?)
}

/// Every recorded split, newest first.
pub fn list_epic_splits(cas_dir: &Path) -> Result<Vec<EpicSplit>> {
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    let mut statement = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM epic_splits ORDER BY created_at DESC, id"
    ))?;
    let rows = statement.query_map([], parse_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Splits that may still bind: pending or accepted.
pub fn open_epic_splits(cas_dir: &Path) -> Result<Vec<EpicSplit>> {
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    let mut statement = conn.prepare(&format!(
        "SELECT {COLUMNS} FROM epic_splits WHERE state IN ('pending', 'accepted')
         ORDER BY created_at DESC, id"
    ))?;
    let rows = statement.query_map([], parse_row)?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
}

/// Move split `id` from one of `from` to `to`, recording `reason` and the
/// decision time. Returns `false` when the split is missing or no longer in
/// one of `from` (another decision won).
pub fn decide_epic_split(
    cas_dir: &Path,
    id: &str,
    from: &[SplitState],
    to: SplitState,
    reason: Option<&str>,
    at: DateTime<Utc>,
) -> Result<bool> {
    if from.is_empty() {
        return Ok(false);
    }
    let conn = open_conn(cas_dir)?;
    let conn = crate::shared_db::lock_connection(&conn)?;
    let states = from
        .iter()
        .map(|state| format!("'{}'", state.as_str()))
        .collect::<Vec<_>>()
        .join(", ");
    let changed = conn.execute(
        &format!(
            "UPDATE epic_splits SET state = ?2, reason = COALESCE(?3, reason), decided_at = ?4
             WHERE id = ?1 AND state IN ({states})"
        ),
        params![id, to.as_str(), reason, at.to_rfc3339()],
    )?;
    Ok(changed == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn split(id: &str) -> EpicSplit {
        let now = Utc::now();
        EpicSplit {
            id: id.to_string(),
            epic_id: "cas-e1".into(),
            local_agent_id: "a".into(),
            peer_agent_id: "b".into(),
            peer_name: "sup-b".into(),
            peer_machine: "beta".into(),
            direction: SplitDirection::Outgoing,
            mine: vec!["cas-1".into(), "cas-2".into()],
            theirs: vec!["cas-3".into()],
            state: SplitState::Pending,
            reason: None,
            message_id: None,
            expires_at: now + chrono::Duration::minutes(30),
            created_at: now,
            decided_at: None,
        }
    }

    #[test]
    fn records_round_trip_and_insert_is_idempotent() {
        let dir = tempfile::TempDir::new().unwrap();
        let original = split("split-1");
        assert!(insert_epic_split(dir.path(), &original).unwrap());
        assert!(!insert_epic_split(dir.path(), &original).unwrap());
        let read = get_epic_split(dir.path(), "split-1").unwrap().unwrap();
        assert_eq!(read.mine, original.mine);
        assert_eq!(read.theirs, original.theirs);
        assert_eq!(read.direction, SplitDirection::Outgoing);
        assert_eq!(read.state, SplitState::Pending);
        assert_eq!(list_epic_splits(dir.path()).unwrap().len(), 1);
        assert!(get_epic_split(dir.path(), "missing").unwrap().is_none());
    }

    #[test]
    fn decisions_are_compare_and_set() {
        let dir = tempfile::TempDir::new().unwrap();
        insert_epic_split(dir.path(), &split("split-1")).unwrap();
        let now = Utc::now();
        let pending = [SplitState::Pending];
        assert!(decide_epic_split(dir.path(), "split-1", &pending, SplitState::Accepted, None, now).unwrap());
        // A second, racing decision from pending loses.
        assert!(!decide_epic_split(dir.path(), "split-1", &pending, SplitState::Rejected, Some("no"), now).unwrap());
        let read = get_epic_split(dir.path(), "split-1").unwrap().unwrap();
        assert_eq!(read.state, SplitState::Accepted);
        assert!(read.decided_at.is_some());
        assert!(open_epic_splits(dir.path()).unwrap().len() == 1);
        assert!(decide_epic_split(
            dir.path(),
            "split-1",
            &[SplitState::Pending, SplitState::Accepted],
            SplitState::Cancelled,
            Some("done"),
            now
        )
        .unwrap());
        assert!(open_epic_splits(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn only_accepted_or_own_unexpired_proposals_bind() {
        let now = Utc::now();
        let mut s = split("split-1");
        assert!(s.is_binding(now));
        s.direction = SplitDirection::Incoming;
        assert!(!s.is_binding(now), "an unanswered incoming proposal binds nothing");
        s.direction = SplitDirection::Outgoing;
        s.expires_at = now - chrono::Duration::seconds(1);
        assert!(!s.is_binding(now));
        assert_eq!(s.display_state(now), "expired");
        s.state = SplitState::Accepted;
        assert!(s.is_binding(now), "acceptance does not expire");
        s.state = SplitState::Cancelled;
        assert!(!s.is_binding(now));
    }
}
