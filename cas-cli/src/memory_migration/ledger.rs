//! The migration ledger — what makes the run resumable and the loss auditable.
//!
//! Resume is keyed on `(db, cas_legacy_id)`, never on row order (AC3): a row
//! inserted into a live source database between two runs must not shift the
//! window of what is considered already-done. Each applied page appends one
//! line and the file is flushed immediately, so a crash costs at most the
//! in-flight row — and re-doing that row is safe because ids and paths are
//! derived deterministically from the legacy id.

use std::collections::HashSet;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const APPLIED_FILE: &str = "applied.jsonl";
pub const QUARANTINE_FILE: &str = "quarantine.jsonl";
pub const AUDIT_FILE: &str = "audit.json";
pub const DEFAULTS_FILE: &str = "legacy-defaults.json";
pub const SYNC_QUEUE_FILE: &str = "sync-queue-invalidated.jsonl";
pub const INDEX_FILE: &str = "page-index.json";

/// One durably-applied legacy row.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppliedRecord {
    pub db: String,
    pub legacy_id: String,
    pub rule: String,
    pub disposition: String,
    pub page_id: String,
    pub rel_path: String,
}

/// A row held back by the R6 contamination predicate. Quarantine is
/// stay-entry-in-place plus this record — never a delete. Deleting another
/// project's records on a heuristic is not a call this migration gets to make.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuarantineRecord {
    pub db: String,
    pub legacy_id: String,
    pub title: String,
    pub matched_token: String,
    pub content: String,
}

#[derive(Debug)]
pub struct Ledger {
    dir: PathBuf,
    applied: HashSet<(String, String)>,
    handle: Option<File>,
}

impl Ledger {
    /// Open (creating if needed) and load the already-applied key set.
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating migration ledger at {}", dir.display()))?;
        let mut applied = HashSet::new();
        let path = dir.join(APPLIED_FILE);
        if path.exists() {
            let file = File::open(&path).with_context(|| format!("reading {}", path.display()))?;
            for line in BufReader::new(file).lines() {
                let line = line?;
                if line.trim().is_empty() {
                    continue;
                }
                // A malformed ledger line must not be skipped into "not yet
                // applied" — that would silently re-run work whose real state
                // is unknown.
                let record: AppliedRecord = serde_json::from_str(&line).with_context(|| {
                    format!("corrupt ledger line in {}: {line}", path.display())
                })?;
                applied.insert((record.db, record.legacy_id));
            }
        }
        Ok(Self {
            dir: dir.to_path_buf(),
            applied,
            handle: None,
        })
    }

    /// Every applied record, in the order the migration wrote them.
    ///
    /// Rollback is driven from this list rather than from a `LIKE 'cas-kn-mig-%'`
    /// sweep of the destination: the ledger is the only record of what THIS
    /// migration created, so a page that merely resembles a migrated one — hand
    /// written, or left by an unrelated run — cannot be caught in the blast.
    pub fn read_applied(dir: &Path) -> Result<Vec<AppliedRecord>> {
        let path = dir.join(APPLIED_FILE);
        if !path.exists() {
            return Ok(Vec::new());
        }
        let file = File::open(&path).with_context(|| format!("reading {}", path.display()))?;
        let mut records = Vec::new();
        for line in BufReader::new(file).lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            records.push(
                serde_json::from_str(&line).with_context(|| {
                    format!("corrupt ledger line in {}: {line}", path.display())
                })?,
            );
        }
        Ok(records)
    }

    pub fn is_applied(&self, db: &str, legacy_id: &str) -> bool {
        self.applied
            .contains(&(db.to_string(), legacy_id.to_string()))
    }

    pub fn applied_count(&self) -> usize {
        self.applied.len()
    }

    /// Append and flush. Flushing per row is the whole point: an unflushed
    /// buffer is indistinguishable from work that never happened.
    pub fn record(&mut self, record: &AppliedRecord) -> Result<()> {
        if self.handle.is_none() {
            let path = self.dir.join(APPLIED_FILE);
            self.handle = Some(
                OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .with_context(|| format!("opening {}", path.display()))?,
            );
        }
        let file = self.handle.as_mut().expect("handle just created");
        writeln!(file, "{}", serde_json::to_string(record)?)?;
        file.flush()?;
        self.applied
            .insert((record.db.clone(), record.legacy_id.clone()));
        Ok(())
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Rewrite a whole-file artifact (audit, quarantine, defaults). These are
    /// snapshots of the current run, not append-only history.
    pub fn write_artifact(&self, name: &str, contents: &str) -> Result<()> {
        let path = self.dir.join(name);
        std::fs::write(&path, contents).with_context(|| format!("writing {}", path.display()))
    }

    pub fn append_artifact(&self, name: &str, contents: &str) -> Result<()> {
        let path = self.dir.join(name);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        file.write_all(contents.as_bytes())?;
        file.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(id: &str) -> AppliedRecord {
        AppliedRecord {
            db: "project".into(),
            legacy_id: id.into(),
            rule: "R7".into(),
            disposition: "migrate-to-page".into(),
            page_id: "cas-kn-mig-abc".into(),
            rel_path: "context/x-abc.md".into(),
        }
    }

    #[test]
    fn applied_rows_survive_a_reopen() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut ledger = Ledger::open(temp.path()).unwrap();
        assert!(!ledger.is_applied("project", "p-1"));
        ledger.record(&record("p-1")).unwrap();
        assert!(ledger.is_applied("project", "p-1"));

        let reopened = Ledger::open(temp.path()).unwrap();
        assert!(reopened.is_applied("project", "p-1"));
        assert!(
            !reopened.is_applied("global", "p-1"),
            "keys are per-database"
        );
        assert_eq!(reopened.applied_count(), 1);
    }

    #[test]
    fn a_corrupt_ledger_line_is_an_error_not_a_silent_rerun() {
        let temp = tempfile::TempDir::new().unwrap();
        std::fs::write(temp.path().join(APPLIED_FILE), "{not json}\n").unwrap();
        let err = Ledger::open(temp.path()).unwrap_err().to_string();
        assert!(err.contains("corrupt ledger line"), "{err}");
    }

    #[test]
    fn blank_lines_are_tolerated() {
        let temp = tempfile::TempDir::new().unwrap();
        let line = serde_json::to_string(&record("p-1")).unwrap();
        std::fs::write(temp.path().join(APPLIED_FILE), format!("\n{line}\n\n")).unwrap();
        assert!(
            Ledger::open(temp.path())
                .unwrap()
                .is_applied("project", "p-1")
        );
    }

    #[test]
    fn artifacts_are_written_beside_the_ledger() {
        let temp = tempfile::TempDir::new().unwrap();
        let ledger = Ledger::open(temp.path()).unwrap();
        ledger.write_artifact(AUDIT_FILE, "{}").unwrap();
        ledger.append_artifact(QUARANTINE_FILE, "a\n").unwrap();
        ledger.append_artifact(QUARANTINE_FILE, "b\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(temp.path().join(QUARANTINE_FILE)).unwrap(),
            "a\nb\n"
        );
    }
}
