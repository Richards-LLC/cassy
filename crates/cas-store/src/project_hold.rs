//! Operator-owned project stop, shared by task visibility and message delivery.
//!
//! Writers live in the operator-only CLI. Reads must remain live (no cache),
//! including on an already-open store, and unreadable state fails closed.
use std::path::Path;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{Result, StoreError};

pub const STATE_FILE: &str = "operator/project-hold.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hold {
    pub reason: String,
    pub operator: String,
    pub set_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Decision {
    pub action: String,
    pub reason: String,
    pub operator: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct State {
    pub active: Option<Hold>,
    pub history: Vec<Decision>,
}

pub fn load(root: &Path) -> Result<State> {
    match std::fs::read(root.join(STATE_FILE)) {
        Ok(bytes) => {
            let state: State = serde_json::from_slice(&bytes)?;
            if state.active.as_ref().is_some_and(|hold| {
                hold.reason.trim().is_empty() || hold.operator.trim().is_empty()
            }) {
                return Err(StoreError::Other(
                    "Project hold has no reason or operator".into(),
                ));
            }
            Ok(state)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error.into()),
    }
}

/// A compact human read; JSON status retains complete operator text and history.
pub fn notice(root: &Path) -> Option<String> {
    match load(root) {
        Ok(State {
            active: Some(hold), ..
        }) => Some(format!(
            "Project hold: {}\nOperator: {}\nSet at: {}\nOnly the operator can lift this hold:\ncas factory hold lift --reason <reason>",
            preview(&hold.reason, 60),
            preview(&hold.operator, 60),
            hold.set_at
                .with_timezone(&chrono::Local)
                .format("%Y-%m-%d %-I:%M %p %Z"),
        )),
        Ok(_) => None,
        Err(error) => Some(format!(
            "Project hold: state unreadable; dispatch and merges are stopped.\n{}\nOperator: inspect `cas --json factory hold status`.",
            preview(&error.to_string(), 76),
        )),
    }
}

pub fn is_held(root: &Path) -> bool {
    load(root).map_or(true, |state| state.active.is_some())
}

/// Connections opened by daemon jobs and caches still identify the same project.
/// In-memory stores have no project hold.
pub(crate) fn held_on_connection(conn: &rusqlite::Connection) -> bool {
    conn.path()
        .filter(|path| !path.is_empty())
        .and_then(|path| Path::new(path).parent())
        .is_some_and(is_held)
}

pub fn preview(value: &str, limit: usize) -> String {
    let clean: String = value
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    if clean.chars().count() > limit {
        format!(
            "{}... (use --json)",
            clean
                .chars()
                .take(limit.saturating_sub(19))
                .collect::<String>()
        )
    } else {
        clean
    }
}

// Prefix and required marker share one definition across memory and SQL
// selection, so held rows cannot monopolize a bounded inbox drain.
const ACTION_RELAYS: &[(&str, &str)] = &[
    ("<task-lifecycle ", "new=\"awaiting_merge\""),
    ("<task-lifecycle ", "transition=\"task_ready\""),
    ("<worker-attention ", "kind=\"supervisor_stalled\""),
    ("<worker-attention ", "kind=\"merged_close_blocked\""),
    ("", "<cas-merge-request>"),
    ("", "<cas-director-ready>"),
    ("<delivery-pr-merged ", ""),
];

/// Match machine relay types, never a free-form request to merge or dispatch.
pub fn is_action_relay(prompt: &str) -> bool {
    ACTION_RELAYS
        .iter()
        .any(|(prefix, marker)| prompt.starts_with(prefix) && prompt.contains(marker))
}

/// Only fixed product literals enter SQL; caller data never does.
pub(crate) fn inbox_exclusion_sql() -> String {
    let rules = ACTION_RELAYS
        .iter()
        .map(|(prefix, marker)| {
            format!("(instr(q.prompt, '{prefix}') = 1 AND instr(q.prompt, '{marker}') > 0)")
        })
        .collect::<Vec<_>>()
        .join(" OR ");
    format!("AND NOT ({rules})")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_hold_corruption_fails_closed_and_no_hold_is_transparent() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_held(dir.path()));
        assert!(notice(dir.path()).is_none());
        std::fs::create_dir_all(dir.path().join("operator")).unwrap();
        std::fs::write(dir.path().join(STATE_FILE), "truncated{").unwrap();
        assert!(is_held(dir.path()));
        assert!(notice(dir.path()).unwrap().contains("state unreadable"));
    }

    #[test]
    fn project_hold_relay_rules_agree_in_sql_and_memory() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        for (body, expected) in [
            (
                "<task-lifecycle transition=\"task_awaiting_merge\" new=\"awaiting_merge\">merge</task-lifecycle>",
                true,
            ),
            (
                "<worker-attention worker=\"supervisor\" kind=\"supervisor_stalled\">dispatch</worker-attention>",
                true,
            ),
            (
                "Merge now\n\n<cas-merge-request>{}</cas-merge-request>",
                true,
            ),
            ("Ready\n\n<cas-director-ready>{}</cas-director-ready>", true),
            (
                "<delivery-pr-merged task_id=\"cas-1\">close</delivery-pr-merged>",
                true,
            ),
            ("Please review whether to merge this", false),
            (
                "<worker-attention kind=\"worker_stalled\">investigate</worker-attention>",
                false,
            ),
            (
                "<task-lifecycle transition=\"task_blocked\" new=\"blocked\">help</task-lifecycle>",
                false,
            ),
        ] {
            assert_eq!(is_action_relay(body), expected, "{body}");
            let sql = format!(
                "SELECT EXISTS(SELECT 1 FROM (SELECT ?1 AS prompt) q WHERE 1=1 {})",
                inbox_exclusion_sql()
            );
            let visible: bool = conn.query_row(&sql, [body], |row| row.get(0)).unwrap();
            assert_eq!(visible, !expected, "{body}");
        }
    }
}
