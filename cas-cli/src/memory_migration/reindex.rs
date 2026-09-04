//! Spec §6 — reindex `knowledge_pages` after migration, and prove it worked.
//!
//! Scope, stated precisely because the two indexes behave very differently:
//!
//! - **`knowledge_pages_fts` (SQLite FTS5, contentless)** is the index the
//!   knowledge retrieval channel actually reads (`HybridSearch` calls
//!   `KnowledgeStore::search`, `hybrid.rs:228`). `commit_ingest` maintains it
//!   inside the same transaction as the page row, so a migrated page is indexed
//!   the moment it is written — but §6 makes the reindex an explicit, logged
//!   step rather than something inferred from that invariant, so this module
//!   rebuilds every page's FTS row from the **body file on disk** (the
//!   authoritative copy) and then verifies each page is retrievable.
//! - **The Tantivy index (`<cas_root>/index`)** is deliberately NOT touched.
//!   `SearchIndex::open` deletes and recreates the index directory on a
//!   field-count mismatch (`search_index_impl.rs:78`), and it holds no
//!   knowledge-page documents at all — `DocType::KnowledgePage` labels hits the
//!   store-backed channel produces, it is not a document type the writer ever
//!   emits. Rebuilding it here would be a destructive no-op for pages and would
//!   invalidate the entries index that `stay-entry` rows still depend on.
//!
//! The verification is not "we ran the writer": each page is probed through a
//! real `MATCH` query constrained to its own rowid, so a page that was written
//! but is not retrievable fails the run.

use std::path::Path;

use anyhow::{Context, Result, bail};
use rusqlite::params;
use serde::Serialize;

#[derive(Debug, Clone, Default, Serialize)]
pub struct PageIndexReport {
    pub cas_root: String,
    /// Pages found in `knowledge_pages`.
    pub pages: usize,
    /// Pages whose FTS row was rebuilt from the on-disk body.
    pub reindexed: usize,
    /// `rel_path`s with a page row but no body file. Hard error: the page is
    /// unreadable, and a migration that produced one has lost the content.
    pub missing_bodies: Vec<String>,
    /// Pages that were indexed but could not be retrieved afterwards. Hard
    /// error — this is the failure mode a "we ran the reindex" log line hides.
    pub unsearchable: Vec<String>,
    /// Pages with no indexable token at all (empty title, snippet and body).
    /// Reported, not failed: there is genuinely nothing to match on.
    pub unverifiable: usize,
}

impl PageIndexReport {
    pub fn check(&self) -> Result<()> {
        if !self.missing_bodies.is_empty() {
            bail!(
                "knowledge reindex [{}]: {} page(s) have no body file on disk: {}",
                self.cas_root,
                self.missing_bodies.len(),
                self.missing_bodies.join(", ")
            );
        }
        if !self.unsearchable.is_empty() {
            bail!(
                "knowledge reindex [{}]: {} page(s) are not retrievable after reindexing: {}",
                self.cas_root,
                self.unsearchable.len(),
                self.unsearchable.join(", ")
            );
        }
        Ok(())
    }

    pub fn render(&self) -> String {
        format!(
            "knowledge page index [{}]: {} page(s), {} reindexed, {} verified, \
             {} with no indexable text\n",
            self.cas_root,
            self.pages,
            self.reindexed,
            self.reindexed.saturating_sub(self.unverifiable),
            self.unverifiable
        )
    }
}

/// First token worth probing the index with: at least three ASCII alphanumerics.
///
/// Shorter runs are skipped because a one- or two-character token is far more
/// likely to be a stopword-ish fragment than a discriminating term.
fn probe_token(candidates: [&str; 3]) -> Option<String> {
    for source in candidates {
        let mut current = String::new();
        for ch in source.chars() {
            if ch.is_ascii_alphanumeric() {
                current.push(ch);
            } else {
                if current.len() >= 3 {
                    return Some(current);
                }
                current.clear();
            }
        }
        if current.len() >= 3 {
            return Some(current);
        }
    }
    None
}

/// Rebuild every page's FTS row from its on-disk body and verify retrievability.
///
/// Runs against `<cas_root>/cas.db` on its own connection: the reindex is a
/// standalone, re-runnable operation the M5 cutover runbook can invoke without
/// re-running the migration.
pub fn reindex_pages(cas_root: &Path) -> Result<PageIndexReport> {
    let db_path = cas_root.join("cas.db");
    let conn = rusqlite::Connection::open(&db_path)
        .with_context(|| format!("opening {} to reindex knowledge pages", db_path.display()))?;

    let has_table: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='knowledge_pages'",
        [],
        |row| row.get(0),
    )?;
    let mut report = PageIndexReport {
        cas_root: cas_root.display().to_string(),
        ..PageIndexReport::default()
    };
    if has_table == 0 {
        return Ok(report);
    }

    let rows: Vec<(i64, String, String, String, String)> = {
        let mut stmt =
            conn.prepare("SELECT row_id, id, title, snippet, rel_path FROM knowledge_pages")?;
        stmt.query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    report.pages = rows.len();

    let knowledge_dir = cas_root.join(cas_store::KNOWLEDGE_DIR_NAME);
    let mut indexed: Vec<(i64, String, String)> = Vec::new();

    // Read every body BEFORE the write transaction opens (cas-759f). Reading
    // them inside it held the store's write lock across N filesystem reads,
    // which on a shared store means every other writer on the host waits on
    // this command's disk IO. The bodies are already durable rows of this same
    // database, so holding them briefly in memory costs nothing the store was
    // not already paying.
    let mut pending: Vec<(i64, &str, &str, &str, String)> = Vec::with_capacity(rows.len());
    for (row_id, id, title, snippet, rel_path) in &rows {
        let body_path = knowledge_dir.join(rel_path);
        match std::fs::read_to_string(&body_path) {
            Ok(body) => pending.push((*row_id, id, title, snippet, body)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.missing_bodies.push(rel_path.clone());
            }
            Err(e) => {
                return Err(e).with_context(|| format!("reading {}", body_path.display()));
            }
        }
    }

    // One transaction: a half-rebuilt FTS index is worse than a stale one.
    // IMMEDIATE rather than the DEFERRED default so the lock is taken where
    // SQLite's busy handler still applies.
    let tx = cas_store::shared_db::begin_immediate_with_retry(&conn)?;
    for (row_id, id, title, snippet, body) in &pending {
        tx.execute(
            "DELETE FROM knowledge_pages_fts WHERE rowid = ?1",
            params![row_id],
        )?;
        tx.execute(
            "INSERT INTO knowledge_pages_fts (rowid, title, snippet, body) VALUES (?1, ?2, ?3, ?4)",
            params![row_id, title, snippet, body],
        )?;
        report.reindexed += 1;
        indexed.push((
            *row_id,
            (*id).to_string(),
            probe_token([*title, *snippet, body.as_str()]).unwrap_or_default(),
        ));
    }
    tx.commit()?;

    // Post-run consistency check: every reindexed page must come back out of a
    // real MATCH query. Constraining by rowid keeps this O(1) per page and
    // immune to a crowded result set.
    for (row_id, id, token) in indexed {
        if token.is_empty() {
            report.unverifiable += 1;
            continue;
        }
        let found: i64 = conn.query_row(
            "SELECT COUNT(*) FROM knowledge_pages_fts
             WHERE knowledge_pages_fts MATCH ?1 AND rowid = ?2",
            params![format!("\"{token}\""), row_id],
            |row| row.get(0),
        )?;
        if found == 0 {
            report.unsearchable.push(id);
        }
    }

    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_token_prefers_the_first_long_enough_run() {
        assert_eq!(probe_token(["a b hooks", "", ""]).as_deref(), Some("hooks"));
        assert_eq!(
            probe_token(["", "snippet text", ""]).as_deref(),
            Some("snippet")
        );
        assert_eq!(probe_token(["", "", "body-only"]).as_deref(), Some("body"));
        assert_eq!(probe_token(["-- ++", "a b", "x y"]), None);
    }

    #[test]
    fn probe_token_handles_a_trailing_run() {
        assert_eq!(probe_token(["ab cde", "", ""]).as_deref(), Some("cde"));
    }

    #[test]
    fn an_absent_knowledge_table_is_not_an_error() {
        let temp = tempfile::TempDir::new().unwrap();
        rusqlite::Connection::open(temp.path().join("cas.db")).unwrap();
        let report = reindex_pages(temp.path()).unwrap();
        assert_eq!(report.pages, 0);
        report.check().unwrap();
    }

    #[test]
    fn a_missing_body_file_is_a_hard_error() {
        let report = PageIndexReport {
            cas_root: "/x".into(),
            pages: 1,
            missing_bodies: vec!["context/a.md".into()],
            ..PageIndexReport::default()
        };
        let err = report.check().unwrap_err().to_string();
        assert!(err.contains("no body file"), "{err}");
    }

    #[test]
    fn an_unsearchable_page_is_a_hard_error() {
        let report = PageIndexReport {
            cas_root: "/x".into(),
            unsearchable: vec!["cas-kn-mig-abc".into()],
            ..PageIndexReport::default()
        };
        let err = report.check().unwrap_err().to_string();
        assert!(err.contains("not retrievable"), "{err}");
    }

    #[test]
    fn a_clean_report_renders_and_passes() {
        let report = PageIndexReport {
            cas_root: "/x".into(),
            pages: 3,
            reindexed: 3,
            ..PageIndexReport::default()
        };
        report.check().unwrap();
        assert!(report.render().contains("3 reindexed"));
    }
}
