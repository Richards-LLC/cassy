//! Ledger-driven rollback (cas-edee / M5 AC1).
//!
//! Returns a Cassy root to its pre-migration state by undoing exactly what this
//! migration did, and nothing else.
//!
//! # Why this is surgical rather than a database restore
//!
//! `cas.db` is not a memory database. On this machine it also holds 1522 tasks,
//! task leases, agents, sessions, verification records, worker delivery
//! receipts, the prompt and supervisor queues and the worktree registry. A
//! factory is writing to all of that continuously. Restoring a pre-migration
//! `cas.db` would therefore not "roll back the migration" — it would roll back
//! the entire system to the backup instant, silently discarding every task
//! closure, verification and message recorded since. That is a larger and much
//! less reversible loss than the one being undone.
//!
//! So the rollback is driven from the ledger: for each page this migration
//! actually created, remove that page. Rows the migration never touched cannot
//! be affected, because they are not in the ledger.
//!
//! # What it refuses to do
//!
//! A ledger record is only acted on if the page currently at that id still has
//! the `rel_path` the ledger recorded. If it does not, the page has been
//! replaced since the migration and the rollback **skips it and reports it**
//! rather than deleting somebody else's work on the strength of a stale id.
//! Silently deleting a page that no longer matches would make the rollback a
//! second data-loss event.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use cas_store::{KnowledgeStore, SqliteKnowledgeStore};
use serde::Serialize;

use super::ledger::{self, AppliedRecord};
use super::{DbLabel, SourceDb};

/// What a rollback did, or — in report-only mode — would do.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct RollbackAudit {
    /// Ledger records considered.
    pub ledgered: usize,
    /// Pages present, matching the ledger, and removed (or removable).
    pub removed: usize,
    /// Pages already gone — a re-run, or a partial rollback resuming.
    pub already_absent: usize,
    /// Pages present whose `rel_path` no longer matches the ledger. NOT
    /// touched. Non-zero means a human has to look.
    pub diverged: Vec<DivergedPage>,
    /// `sync_queue` rows restored from the invalidation ledger.
    pub sync_queue_restored: usize,
    /// Body files removed from disk.
    pub bodies_removed: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DivergedPage {
    pub page_id: String,
    pub ledger_rel_path: String,
    pub current_rel_path: String,
}

impl RollbackAudit {
    /// The rollback is complete when nothing the ledger claims is still
    /// standing. `diverged` deliberately does NOT count as complete.
    pub fn is_clean(&self) -> bool {
        self.diverged.is_empty() && self.removed + self.already_absent == self.ledgered
    }

    pub fn render(&self, applied: bool) -> String {
        let mut out = String::new();
        out.push_str(if applied {
            "rollback\n"
        } else {
            "rollback PLAN (report only — pass --apply to execute)\n"
        });
        out.push_str("─────────────────────────────────────────────\n");
        for (label, count) in [
            ("ledgered pages", self.ledgered),
            (
                if applied {
                    "pages removed"
                } else {
                    "pages to remove"
                },
                self.removed,
            ),
            ("body files removed", self.bodies_removed),
            ("already absent", self.already_absent),
            ("diverged (NOT touched)", self.diverged.len()),
            ("sync_queue rows restored", self.sync_queue_restored),
        ] {
            out.push_str(&format!("{label:<34}{count:>8}\n"));
        }
        out.push_str("─────────────────────────────────────────────\n");
        if self.diverged.is_empty() {
            out.push_str(if self.is_clean() {
                "state: every ledgered page accounted for\n"
            } else {
                "state: INCOMPLETE — see counts above\n"
            });
        } else {
            out.push_str(
                "state: DIVERGED — these pages were replaced after the migration and were \
                 left alone. Resolve by hand; do not assume the rollback is done.\n",
            );
            for page in &self.diverged {
                out.push_str(&format!(
                    "  {} ledger={} current={}\n",
                    page.page_id, page.ledger_rel_path, page.current_rel_path
                ));
            }
        }
        out
    }
}

/// Roll back the pages recorded in `ledger_dir`.
///
/// `apply` false is a full dry run: every check runs, nothing is deleted.
pub fn rollback(sources: &[SourceDb], ledger_dir: &Path, apply: bool) -> Result<RollbackAudit> {
    let records = ledger::Ledger::read_applied(ledger_dir)?;
    let mut audit = RollbackAudit {
        ledgered: records.len(),
        ..Default::default()
    };

    for source in sources {
        let store = SqliteKnowledgeStore::open(&source.cas_root)
            .with_context(|| format!("opening knowledge store at {}", source.cas_root.display()))?;
        let label = source.label.as_str();
        for record in records.iter().filter(|r| r.db == label) {
            match classify(&store, record)? {
                Standing::Absent => audit.already_absent += 1,
                Standing::Diverged(current_rel_path) => audit.diverged.push(DivergedPage {
                    page_id: record.page_id.clone(),
                    ledger_rel_path: record.rel_path.clone(),
                    current_rel_path,
                }),
                Standing::Matches => {
                    let body = body_path(&source.cas_root, &record.rel_path);
                    let body_existed = body.exists();
                    if apply {
                        store
                            .delete_page(&record.page_id)
                            .with_context(|| format!("rolling back page {}", record.page_id))?;
                        // delete_page removes the body too; count what was
                        // actually there rather than what we asked for.
                        if body_existed && !body.exists() {
                            audit.bodies_removed += 1;
                        }
                    } else if body_existed {
                        audit.bodies_removed += 1;
                    }
                    audit.removed += 1;
                }
            }
        }
    }

    audit.sync_queue_restored = restore_sync_queue(sources, ledger_dir, apply)?;
    Ok(audit)
}

enum Standing {
    Absent,
    Matches,
    Diverged(String),
}

fn classify(store: &SqliteKnowledgeStore, record: &AppliedRecord) -> Result<Standing> {
    // `get_page` returns NotFound as an error rather than `None`, so a missing
    // page is discriminated by error kind, not by an Option. Anything that is
    // NOT a not-found is a real failure and must propagate — swallowing it
    // would let a locked or corrupt store read as "already rolled back".
    match store.get_page(&record.page_id) {
        Ok(page) if page.rel_path == record.rel_path => Ok(Standing::Matches),
        Ok(page) => Ok(Standing::Diverged(page.rel_path)),
        Err(cas_store::StoreError::NotFound(_)) => Ok(Standing::Absent),
        Err(err) => Err(anyhow::anyhow!(
            "reading page {} during rollback: {err}",
            record.page_id
        )),
    }
}

fn body_path(cas_root: &Path, rel_path: &str) -> PathBuf {
    cas_root.join("knowledge").join(rel_path)
}

/// Put back the `sync_queue` rows `--invalidate-sync-queue` deleted.
///
/// The payloads were ledgered in full BEFORE the delete precisely so this is
/// possible. Restoration is by `INSERT OR IGNORE` on the recorded columns, so
/// running it twice — or after the operator already drained the queue through
/// `cas cloud sync` — cannot duplicate a row.
fn restore_sync_queue(sources: &[SourceDb], ledger_dir: &Path, apply: bool) -> Result<usize> {
    let path = ledger_dir.join(ledger::SYNC_QUEUE_FILE);
    if !path.exists() {
        return Ok(0);
    }
    let contents =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let rows: Vec<serde_json::Value> = contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(serde_json::from_str)
        .collect::<std::result::Result<_, _>>()
        .with_context(|| format!("corrupt sync_queue ledger in {}", path.display()))?;
    if rows.is_empty() {
        return Ok(0);
    }

    // Route each payload back to the database it came from. The migration
    // stamps `cas_migration_db` for exactly this reason: restoring a global
    // row into the project store would not be a rollback, it would be a second
    // corruption wearing a rollback's name.
    let mut restored = 0usize;
    for source in sources {
        let label = source.label.as_str();
        let mine: Vec<&serde_json::Value> = rows
            .iter()
            .filter(|r| r.get("cas_migration_db").and_then(|v| v.as_str()) == Some(label))
            .filter_map(|r| r.get("row"))
            .collect();
        if mine.is_empty() {
            continue;
        }
        if !apply {
            restored += mine.len();
            continue;
        }
        let conn = rusqlite::Connection::open(&source.db_path).with_context(|| {
            format!("opening {} to restore sync_queue", source.db_path.display())
        })?;
        let table_exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sync_queue'",
            [],
            |row| row.get(0),
        )?;
        if table_exists == 0 {
            anyhow::bail!(
                "{} rows were invalidated from the {label} database but it has no sync_queue \
                 table to restore them into — refusing to write them elsewhere",
                mine.len()
            );
        }
        for row in mine {
            let Some(object) = row.as_object() else {
                continue;
            };
            let columns: Vec<&String> = object.keys().collect();
            if columns.is_empty() {
                continue;
            }
            let placeholders = (1..=columns.len())
                .map(|i| format!("?{i}"))
                .collect::<Vec<_>>()
                .join(", ");
            let column_list = columns
                .iter()
                .map(|c| format!("\"{c}\""))
                .collect::<Vec<_>>()
                .join(", ");
            let values: Vec<rusqlite::types::Value> =
                columns.iter().map(|c| json_to_sql(&object[*c])).collect();
            let sql =
                format!("INSERT OR IGNORE INTO sync_queue ({column_list}) VALUES ({placeholders})");
            restored += conn.execute(&sql, rusqlite::params_from_iter(values.iter()))?;
        }
    }

    // An unstamped ledger predates the routing fix. Guessing a destination is
    // how the rows ended up in the wrong database in the first place.
    let unstamped = rows
        .iter()
        .filter(|r| r.get("cas_migration_db").is_none())
        .count();
    if unstamped > 0 {
        anyhow::bail!(
            "{unstamped} sync_queue payload(s) in {} carry no `cas_migration_db` stamp, so the \
             database they belong to is unknown. They were NOT restored — restoring them into a \
             guessed root would move another store's queue. Re-create the ledger, or replay them \
             by hand against the correct database.",
            path.display()
        );
    }
    Ok(restored)
}

fn json_to_sql(value: &serde_json::Value) -> rusqlite::types::Value {
    use rusqlite::types::Value;
    match value {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Integer(i64::from(*b)),
        serde_json::Value::Number(n) => n
            .as_i64()
            .map(Value::Integer)
            .or_else(|| n.as_f64().map(Value::Real))
            .unwrap_or(Value::Null),
        serde_json::Value::String(s) => Value::Text(s.clone()),
        other => Value::Text(other.to_string()),
    }
}

/// Convenience for the CLI: the source set a rollback needs is the same shape
/// the migration used, minus anything that has no database on disk.
pub fn reachable_sources(sources: &[SourceDb]) -> Vec<SourceDb> {
    sources
        .iter()
        .filter(|s| s.db_path.exists())
        .cloned()
        .collect()
}

/// Label helper so the CLI can report which roots a rollback will touch.
pub fn describe(sources: &[SourceDb]) -> Vec<(DbLabel, PathBuf)> {
    sources
        .iter()
        .map(|s| (s.label, s.cas_root.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_ledger_rolls_back_nothing_and_is_clean() {
        let temp = tempfile::TempDir::new().unwrap();
        let audit = rollback(&[], temp.path(), false).unwrap();
        assert_eq!(audit.ledgered, 0);
        assert!(audit.is_clean());
    }

    #[test]
    fn a_diverged_page_makes_the_audit_unclean_even_with_nothing_left_to_do() {
        let audit = RollbackAudit {
            ledgered: 1,
            removed: 0,
            already_absent: 1,
            diverged: vec![DivergedPage {
                page_id: "cas-kn-mig-a".into(),
                ledger_rel_path: "context/a-a.md".into(),
                current_rel_path: "context/b-a.md".into(),
            }],
            ..Default::default()
        };
        assert!(!audit.is_clean(), "divergence must not read as success");
        let report = audit.render(true);
        assert!(report.contains("DIVERGED"), "{report}");
        assert!(report.contains("cas-kn-mig-a"), "{report}");
    }

    #[test]
    fn the_dry_run_report_says_it_changed_nothing() {
        let report = RollbackAudit::default().render(false);
        assert!(report.contains("report only"), "{report}");
        assert!(report.contains("--apply"), "{report}");
    }

    #[test]
    fn json_scalars_map_onto_sqlite_types() {
        use rusqlite::types::Value;
        assert_eq!(json_to_sql(&serde_json::Value::Null), Value::Null);
        assert_eq!(json_to_sql(&serde_json::json!(7)), Value::Integer(7));
        assert_eq!(
            json_to_sql(&serde_json::json!("x")),
            Value::Text("x".into())
        );
        assert_eq!(json_to_sql(&serde_json::json!(true)), Value::Integer(1));
    }
}
