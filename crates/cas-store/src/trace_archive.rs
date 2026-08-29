//! Immutable compressed archives for daemon event and recording rows.
//!
//! Each archive operation creates a new zstd-compressed JSONL file.  Files are
//! never opened in append or update mode, so a later maintenance run cannot
//! rewrite an earlier archive.  The JSONL format keeps individual records
//! recoverable without making the archive a live query surface.

use cas_types::{Recording, RecordingAgent, RecordingEvent};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::Result;

const TRACE_ARCHIVE_EXTENSION: &str = ".jsonl.zst";
const TRACE_ARCHIVE_DIR: &str = "archive";
static ARCHIVE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// A recording row and its relational children as one archive record.
///
/// FTS rows are retained as searchable transcript payload, while the live
/// index remains a derived structure that a future archive reader may rebuild.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingArchive {
    pub recording: Recording,
    pub agents: Vec<RecordingAgent>,
    pub events: Vec<RecordingEvent>,
    pub fts: Vec<RecordingFtsEntry>,
}

/// Searchable transcript content associated with a recording.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingFtsEntry {
    pub recording_id: String,
    pub content: String,
    pub content_type: String,
    pub timestamp_ms: i64,
}

/// Size and file count for immutable event/recording archives.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TraceArchiveStats {
    pub files: usize,
    pub bytes: u64,
}

/// Write serialized records to a newly-created compressed JSONL archive.
///
/// The file is opened with `create_new`, and a collision gets a sequence
/// suffix instead of modifying the existing file.  Empty batches are rejected
/// because an empty archive carries no durable trace information.
pub fn write_jsonl_archive<T: Serialize>(
    archive_dir: &Path,
    prefix: &str,
    records: &[T],
) -> Result<PathBuf> {
    if records.is_empty() {
        return Err(crate::StoreError::Other(
            "cannot create an empty trace archive".to_string(),
        ));
    }

    fs::create_dir_all(archive_dir)?;
    let now = Utc::now();
    let sequence = ARCHIVE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let stem = format!(
        "{prefix}-{}-{:09}-{}-{sequence}",
        now.timestamp(),
        now.timestamp_subsec_nanos(),
        std::process::id()
    );

    for collision in 0..100u32 {
        let suffix = if collision == 0 {
            String::new()
        } else {
            format!("-{collision}")
        };
        let path = archive_dir.join(format!("{stem}{suffix}{TRACE_ARCHIVE_EXTENSION}"));
        let file = match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.into()),
        };

        let result = write_archive_file(file, records);
        if result.is_err() {
            // A failed write must never leave a partial archive that looks
            // complete to a later retention/reporting pass.
            let _ = fs::remove_file(&path);
        }
        return result.map(|_| path);
    }

    Err(crate::StoreError::Other(format!(
        "could not allocate a unique trace archive path for {prefix}"
    )))
}

fn write_archive_file<T: Serialize>(mut file: File, records: &[T]) -> Result<()> {
    let mut encoder = zstd::stream::write::Encoder::new(file, 3)?;
    for record in records {
        serde_json::to_writer(&mut encoder, record)?;
        encoder.write_all(b"\n")?;
    }
    file = encoder.finish()?;
    file.sync_all()?;
    Ok(())
}

/// Return the size and count of event/recording archive files under a CAS root.
pub fn trace_archive_stats(cas_root: &Path) -> Result<TraceArchiveStats> {
    let archive_dir = cas_root.join(TRACE_ARCHIVE_DIR);
    let entries = match fs::read_dir(archive_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TraceArchiveStats::default());
        }
        Err(error) => return Err(error.into()),
    };

    let mut stats = TraceArchiveStats::default();
    for entry in entries {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() && is_trace_archive_name(&entry.file_name()) {
            stats.files += 1;
            stats.bytes = stats.bytes.saturating_add(metadata.len());
        }
    }
    Ok(stats)
}

/// Remove archived trace files older than the configured retention period.
///
/// A value of zero means keep forever.  Retention is based on the immutable
/// archive file's modification time, not on any mutable database state.
pub fn prune_trace_archives(cas_root: &Path, retention_days: u64) -> Result<usize> {
    if retention_days == 0 {
        return Ok(0);
    }

    let archive_dir = cas_root.join(TRACE_ARCHIVE_DIR);
    let entries = match fs::read_dir(archive_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let age = std::time::Duration::from_secs(retention_days.saturating_mul(86_400));
    let cutoff = SystemTime::now()
        .checked_sub(age)
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let mut removed = 0;

    for entry in entries {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() || !is_trace_archive_name(&entry.file_name()) {
            continue;
        }
        if metadata
            .modified()
            .map(|modified| modified < cutoff)
            .unwrap_or(false)
        {
            fs::remove_file(entry.path())?;
            removed += 1;
        }
    }
    Ok(removed)
}

fn is_trace_archive_name(name: &std::ffi::OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    (name.starts_with("events-") || name.starts_with("recordings-"))
        && name.ends_with(TRACE_ARCHIVE_EXTENSION)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::io::Read;
    use tempfile::TempDir;

    #[test]
    fn archives_are_write_once_and_compressed_jsonl() {
        let temp = TempDir::new().unwrap();
        let archive_dir = temp.path().join("archive");
        let first = write_jsonl_archive(
            &archive_dir,
            "events",
            &[serde_json::json!({"id": 1, "summary": "first"})],
        )
        .unwrap();
        let first_bytes = fs::read(&first).unwrap();
        let second = write_jsonl_archive(
            &archive_dir,
            "events",
            &[serde_json::json!({"id": 2, "summary": "second"})],
        )
        .unwrap();

        assert_ne!(first, second);
        assert_eq!(first_bytes, fs::read(&first).unwrap());

        let mut decoded = String::new();
        zstd::stream::read::Decoder::new(File::open(&second).unwrap())
            .unwrap()
            .read_to_string(&mut decoded)
            .unwrap();
        let record: Value = serde_json::from_str(decoded.trim()).unwrap();
        assert_eq!(record["summary"], "second");
        let stats = trace_archive_stats(temp.path()).unwrap();
        assert_eq!(stats.files, 2);
        assert_eq!(
            stats.bytes,
            first_bytes.len() as u64 + fs::metadata(second).unwrap().len()
        );
    }

    #[test]
    fn archive_retention_zero_keeps_files_and_positive_days_removes_old_files() {
        let temp = TempDir::new().unwrap();
        let archive_dir = temp.path().join("archive");
        let old = write_jsonl_archive(
            &archive_dir,
            "events",
            &[serde_json::json!({"id": 1})],
        )
        .unwrap();
        let recent = write_jsonl_archive(
            &archive_dir,
            "recordings",
            &[serde_json::json!({"id": 2})],
        )
        .unwrap();

        assert_eq!(prune_trace_archives(temp.path(), 0).unwrap(), 0);
        assert!(old.exists());
        assert!(recent.exists());

        let old_time = SystemTime::now() - std::time::Duration::from_secs(2 * 86_400);
        File::open(&old).unwrap().set_modified(old_time).unwrap();
        assert_eq!(prune_trace_archives(temp.path(), 1).unwrap(), 1);
        assert!(!old.exists());
        assert!(recent.exists());
    }
}
