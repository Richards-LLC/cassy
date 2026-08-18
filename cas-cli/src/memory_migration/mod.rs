//! Legacy memory → knowledge-page migration (cas-f4c1, M3 of EPIC cas-b129).
//!
//! Implements `docs/migration/cas-b129-mapping-spec.md`, which is normative.
//! The four properties the epic asked for map onto the code like this:
//!
//! - **dry-run first** — [`MigrationConfig::apply`] defaults to `false`
//!   everywhere it is constructed; the dry run performs the full extraction,
//!   routing and audit and writes only ledger artifacts. Nothing reaches the
//!   knowledge store without an explicit flag.
//! - **zero-loss audit** — every extracted row is routed by the total ordered
//!   procedure in [`routing::route`] and counted into [`audit::LossAudit`]. An
//!   unrouted row aborts; an unbalanced audit aborts. Both are errors, not
//!   warnings.
//! - **idempotent** — page ids and `rel_path`s are derived deterministically
//!   from the legacy id, so re-running converges on the same rows instead of
//!   duplicating them.
//! - **resumable** — the ledger is keyed on `(db, cas_legacy_id)` and flushed
//!   per row, so a crash resumes rather than restarts, including the awkward
//!   case where the crash landed between locking a page and recording it.
//!
//! Both source databases are opened **read-only**; the only writes this module
//! performs are to the destination knowledge store, the ledger directory, and —
//! only when explicitly asked — the `sync_queue` invalidation of §5.3.

pub mod apply;
pub mod audit;
pub mod frontmatter;
pub mod ledger;
pub mod preconditions;
pub mod reindex;
pub mod rollback;
pub mod routing;
pub mod source;

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use cas_store::SqliteKnowledgeStore;

pub use apply::{body_after_frontmatter, compose_body};
pub use audit::LossAudit;
pub use ledger::{AppliedRecord, QuarantineRecord};
pub use reindex::PageIndexReport;
pub use rollback::{RollbackAudit, rollback};
pub use routing::Disposition;

/// Which legacy database a row came from. Also the `cas_legacy_db` value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DbLabel {
    Global,
    Project,
}

impl DbLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::Project => "project",
        }
    }
}

/// One legacy database and the Cassy root it belongs to.
///
/// The destination is the knowledge store of the row's **own** root: global
/// memories become global knowledge pages and project memories become project
/// ones, preserving the scope split Cassy already has.
#[derive(Debug, Clone)]
pub struct SourceDb {
    pub label: DbLabel,
    pub db_path: PathBuf,
    pub cas_root: PathBuf,
}

#[derive(Debug, Clone)]
pub struct MigrationConfig {
    pub sources: Vec<SourceDb>,
    pub ledger_dir: PathBuf,
    /// `false` = report only. This is the default at every construction site.
    pub apply: bool,
    /// §5.3 step 3: ledger the stranded `sync_queue` payloads, then delete them.
    pub invalidate_sync_queue: bool,
    pub page_size: usize,
    /// Spec §6: rebuild every knowledge page's FTS row from its on-disk body
    /// and verify each page is retrievable afterwards. Independent of `apply`,
    /// so the M5 cutover runbook can invoke the reindex as its own step.
    pub reindex: bool,
    /// Stop after this many pages. Exists so the resumability test can produce
    /// exactly the on-disk state a crashed run leaves behind.
    pub stop_after: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct MigrationOutcome {
    pub audit: LossAudit,
    pub quarantine: Vec<QuarantineRecord>,
    /// Pages written by *this* run.
    pub applied: usize,
    /// Page-producing rows a previous run already completed.
    pub skipped_already_applied: usize,
    /// Stranded `sync_queue` entry rows still outstanding when the run ended.
    /// Zero once `invalidate_sync_queue` has consumed them, so the operator is
    /// never told to resolve a queue this run already ledgered and drained.
    pub sync_queue_pending: i64,
    /// Rows whose full payloads were written to the ledger and then deleted.
    pub sync_queue_invalidated: i64,
    pub ledger_dir: PathBuf,
    /// Populated when `apply` stopped early because `stop_after` was reached.
    pub stopped_early: bool,
    /// One report per destination store, when the §6 reindex ran.
    pub index_reports: Vec<PageIndexReport>,
}

struct Extracted {
    label: DbLabel,
    cas_root: PathBuf,
    rows: Vec<source::LegacyRow>,
}

/// Human-readable quarantine report: id, title and matched token per row.
///
/// Supervisor ruling on escalation E1: the dry run MUST print this list in full
/// for review before any apply is authorized.
pub fn render_quarantine(records: &[QuarantineRecord]) -> String {
    if records.is_empty() {
        return "contamination quarantine: none\n".to_string();
    }
    let mut out = format!(
        "contamination quarantine (R6) — {} row(s) held back, NOT deleted\n",
        records.len()
    );
    out.push_str("these rows stay in `entries` in place; review before applying\n");
    out.push_str("─────────────────────────────────────────────────────────────\n");
    for record in records {
        out.push_str(&format!(
            "  [{}] {:<12} token={:<14} {}\n",
            record.db, record.legacy_id, record.matched_token, record.title
        ));
    }
    out
}

/// Run the migration. Dry-run unless `config.apply`.
pub fn run(config: &MigrationConfig) -> Result<MigrationOutcome> {
    if config.sources.is_empty() {
        bail!("no legacy source databases configured");
    }
    let mut ledger = ledger::Ledger::open(&config.ledger_dir)?;
    ledger.write_artifact(
        ledger::DEFAULTS_FILE,
        &serde_json::to_string_pretty(&serde_json::json!({
            "note": "Rule C3 — a cas_legacy_* key is omitted when the source column \
                     holds the DDL default below. Absence decodes to these values.",
            "defaults": frontmatter::DEFAULTS
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<std::collections::BTreeMap<_, _>>(),
        }))?,
    )?;

    // ── Phase A — preconditions, before anything is read for migration ──
    let mut sync_queue_pending = 0i64;
    for db in &config.sources {
        let conn = source::open_read_only(&db.db_path)?;
        preconditions::check_structural(&conn, &db.cas_root, db.label.as_str())?;
        sync_queue_pending += preconditions::sync_queue_entry_count(&conn)?;
    }

    // ── Phase B — §5.3: the queue must be empty before extraction ───────
    let mut sync_queue_invalidated = 0i64;
    if config.apply && sync_queue_pending > 0 {
        if !config.invalidate_sync_queue {
            bail!(
                "{sync_queue_pending} stranded sync_queue entry row(s) must be resolved \
                 before applying (spec §5.3). Drain them first with `cas cloud sync`, or \
                 re-run with --invalidate-sync-queue to write their full payloads to the \
                 ledger and delete them. They are never preserved across the migration: \
                 they reference legacy ids the routing rules may re-home, and replaying \
                 them afterwards would re-create the cross-project contamination this \
                 corpus already shows."
            );
        }
        for db in &config.sources {
            invalidate_sync_queue(db, &ledger)?;
        }
        // Re-count rather than assume the delete matched the pre-count: the
        // remainder is what an operator would still have to resolve, and
        // claiming zero without looking is how a silent drop gets reported as
        // a success.
        sync_queue_invalidated = sync_queue_pending;
        sync_queue_pending = 0;
        for db in &config.sources {
            let conn = source::open_read_only(&db.db_path)?;
            sync_queue_pending += preconditions::sync_queue_entry_count(&conn)?;
        }
        sync_queue_invalidated -= sync_queue_pending;
    }

    // ── Phase C — extraction (read-only, keyset-paginated) ──────────────
    let mut extracted: Vec<Extracted> = Vec::new();
    for db in &config.sources {
        let conn = source::open_read_only(&db.db_path)?;
        let before = source::count_entries(&conn)?;
        let rows = source::extract_all(&conn, config.page_size)?;
        let after = source::count_entries(&conn)?;
        preconditions::assert_stable_count(before, after, db.label.as_str())?;
        if rows.len() as i64 != before {
            bail!(
                "[{}] extraction returned {} row(s) but the table holds {before}; \
                 the migration aborts rather than audit a corpus it did not fully read",
                db.label.as_str(),
                rows.len()
            );
        }
        extracted.push(Extracted {
            label: db.label,
            cas_root: db.cas_root.clone(),
            rows,
        });
    }

    // ── Phase D — routing and the loss audit ────────────────────────────
    let mut loss = LossAudit::default();
    let mut quarantine: Vec<QuarantineRecord> = Vec::new();
    // (source index, row index, route) for everything that produces a page.
    let mut page_plan: Vec<(usize, usize, routing::Route)> = Vec::new();

    for (db_index, batch) in extracted.iter().enumerate() {
        for (row_index, row) in batch.rows.iter().enumerate() {
            let route = routing::route(row)?;
            loss.record(batch.label.as_str(), route.rule, route.disposition);
            if let Some(token) = &route.quarantine_token {
                quarantine.push(QuarantineRecord {
                    db: batch.label.as_str().to_string(),
                    legacy_id: row.id.clone(),
                    title: row.display_title(),
                    matched_token: token.clone(),
                    content: row.content.clone(),
                });
            }
            if route.disposition.writes_a_page() {
                page_plan.push((db_index, row_index, route));
            }
        }
    }
    loss.check()?;

    let quarantine_jsonl = quarantine
        .iter()
        .map(|record| Ok(format!("{}\n", serde_json::to_string(record)?)))
        .collect::<Result<String>>()?;
    ledger.write_artifact(ledger::QUARANTINE_FILE, &quarantine_jsonl)?;
    ledger.write_artifact(ledger::AUDIT_FILE, &serde_json::to_string_pretty(&loss)?)?;

    let mut outcome = MigrationOutcome {
        audit: loss,
        quarantine,
        applied: 0,
        skipped_already_applied: 0,
        sync_queue_pending,
        sync_queue_invalidated,
        ledger_dir: config.ledger_dir.clone(),
        stopped_early: false,
        index_reports: Vec::new(),
    };
    if !config.apply {
        // §6 is an operation the migration owns, not a side effect of applying:
        // the cutover runbook may invoke it on its own after an earlier apply.
        if config.reindex {
            run_reindex(config, &mut outcome, &mut ledger)?;
        }
        return Ok(outcome);
    }

    // ── Phase E — write pages ───────────────────────────────────────────
    let mut stores: Vec<Option<SqliteKnowledgeStore>> = Vec::new();
    for batch in &extracted {
        stores.push(Some(
            SqliteKnowledgeStore::open(&batch.cas_root).with_context(|| {
                format!("opening knowledge store at {}", batch.cas_root.display())
            })?,
        ));
    }

    for (db_index, row_index, route) in page_plan {
        let batch = &extracted[db_index];
        let row = &batch.rows[row_index];
        if ledger.is_applied(batch.label.as_str(), &row.id) {
            outcome.skipped_already_applied += 1;
            continue;
        }
        if let Some(limit) = config.stop_after {
            if outcome.applied >= limit {
                outcome.stopped_early = true;
                break;
            }
        }
        let store = stores[db_index].as_ref().expect("store opened above");
        let prepared = apply::prepare(row, batch.label, route.disposition);
        let page_id = apply::write_page(store, &prepared)
            .with_context(|| format!("migrating legacy row {}", row.id))?;
        ledger.record(&AppliedRecord {
            db: batch.label.as_str().to_string(),
            legacy_id: row.id.clone(),
            rule: route.rule.to_string(),
            disposition: route.disposition.as_str().to_string(),
            page_id,
            rel_path: prepared.page.rel_path.clone(),
        })?;
        outcome.applied += 1;
    }

    if config.reindex {
        run_reindex(config, &mut outcome, &mut ledger)?;
    }

    Ok(outcome)
}

/// §6 — reindex every destination store's pages and verify the result.
///
/// Each report is checked immediately: a page that is written but not
/// retrievable fails the run rather than being reported and shrugged at.
fn run_reindex(
    config: &MigrationConfig,
    outcome: &mut MigrationOutcome,
    ledger: &mut ledger::Ledger,
) -> Result<()> {
    let mut roots: Vec<PathBuf> = config
        .sources
        .iter()
        .map(|db| db.cas_root.clone())
        .collect();
    roots.sort();
    roots.dedup();
    for root in roots {
        let report = reindex::reindex_pages(&root)?;
        report.check()?;
        outcome.index_reports.push(report);
    }
    ledger.write_artifact(
        ledger::INDEX_FILE,
        &serde_json::to_string_pretty(&outcome.index_reports)?,
    )?;
    Ok(())
}

/// §5.3 step 3 — invalidate, but only *after* the payloads are ledgered.
fn invalidate_sync_queue(db: &SourceDb, ledger: &ledger::Ledger) -> Result<()> {
    let conn = rusqlite::Connection::open(&db.db_path)
        .with_context(|| format!("opening {} to invalidate sync_queue", db.db_path.display()))?;
    let exists: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='sync_queue'",
        [],
        |row| row.get(0),
    )?;
    if exists == 0 {
        return Ok(());
    }

    let column_names: Vec<String> = {
        let stmt = conn.prepare("SELECT * FROM sync_queue LIMIT 0")?;
        stmt.column_names()
            .into_iter()
            .map(ToString::to_string)
            .collect()
    };
    let mut stmt = conn.prepare("SELECT * FROM sync_queue WHERE entity_type = 'entry'")?;
    let payloads = stmt.query_map([], |row| {
        let mut object = serde_json::Map::new();
        for (index, name) in column_names.iter().enumerate() {
            let value = match row.get_ref(index)? {
                rusqlite::types::ValueRef::Null => serde_json::Value::Null,
                rusqlite::types::ValueRef::Integer(v) => serde_json::Value::from(v),
                rusqlite::types::ValueRef::Real(v) => serde_json::Value::from(v),
                rusqlite::types::ValueRef::Text(v) => {
                    serde_json::Value::from(String::from_utf8_lossy(v).to_string())
                }
                rusqlite::types::ValueRef::Blob(v) => serde_json::Value::from(hex_of(v)),
            };
            object.insert(name.clone(), value);
        }
        Ok(serde_json::Value::Object(object))
    })?;

    // Each payload records WHICH database it came from. Without that, a
    // rollback has no way to route the rows home and would restore them into
    // whichever root it happened to try first — silently moving another store's
    // queue. (Caught by exercising the rollback: 11 global rows came back into
    // the project database.)
    let mut recorded = String::new();
    for payload in payloads {
        let record = serde_json::json!({
            "cas_migration_db": db.label.as_str(),
            "row": payload?,
        });
        recorded.push_str(&format!("{}\n", serde_json::to_string(&record)?));
    }
    drop(stmt);
    if recorded.is_empty() {
        return Ok(());
    }
    // Ledger first. A payload deleted before it is durable is exactly the
    // silent drop this migration is not allowed to make.
    ledger.append_artifact(ledger::SYNC_QUEUE_FILE, &recorded)?;
    conn.execute("DELETE FROM sync_queue WHERE entity_type = 'entry'", [])?;
    Ok(())
}

fn hex_of(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_labels_are_the_cas_legacy_db_values() {
        assert_eq!(DbLabel::Global.as_str(), "global");
        assert_eq!(DbLabel::Project.as_str(), "project");
    }

    #[test]
    fn an_empty_quarantine_still_renders_a_line() {
        assert!(render_quarantine(&[]).contains("none"));
    }

    #[test]
    fn the_quarantine_report_names_id_title_and_token() {
        let report = render_quarantine(&[QuarantineRecord {
            db: "project".into(),
            legacy_id: "p-q1".into(),
            title: "FINAL QBO State".into(),
            matched_token: "QBO".into(),
            content: "44 JEs".into(),
        }]);
        assert!(report.contains("p-q1"));
        assert!(report.contains("FINAL QBO State"));
        assert!(report.contains("QBO"));
        assert!(report.contains("NOT deleted"));
    }

    #[test]
    fn a_run_with_no_sources_is_an_error() {
        let temp = tempfile::TempDir::new().unwrap();
        let config = MigrationConfig {
            sources: Vec::new(),
            ledger_dir: temp.path().to_path_buf(),
            apply: false,
            invalidate_sync_queue: false,
            page_size: 500,
            reindex: false,
            stop_after: None,
        };
        assert!(run(&config).is_err());
    }
}
