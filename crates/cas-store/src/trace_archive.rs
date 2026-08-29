//! Immutable compressed archives for daemon event and recording rows.
//!
//! Each archive operation creates a new zstd-compressed JSONL file.  Files are
//! never opened in append or update mode, so a later maintenance run cannot
//! rewrite an earlier archive.  The JSONL format keeps individual records
//! recoverable without making the archive a live query surface.

use cas_types::{Event, Recording, RecordingAgent, RecordingEvent};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use crate::Result;

const TRACE_ARCHIVE_EXTENSION: &str = ".jsonl.zst";
const TRACE_ARCHIVE_DIR: &str = "archive";

/// Default total on-disk size for immutable event and recording archives.
///
/// A finite default is intentional: archive retention is size-bounded even
/// when users do not add a daemon section to their config file.
pub const DEFAULT_TRACE_ARCHIVE_MAX_BYTES: u64 = 1024 * 1024 * 1024;

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

/// Result of enforcing the trace archive size cap.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TraceArchiveEviction {
    /// Number of oldest archive files removed.
    pub files_evicted: usize,
    /// Compressed bytes removed from the archive directory.
    pub bytes_evicted: u64,
    /// Compressed bytes remaining after eviction.
    pub remaining_bytes: u64,
}

/// One trace recovered from an immutable archive file.
#[derive(Debug, Clone)]
pub enum ArchivedTraceRecord {
    Event(Event),
    Recording(RecordingArchive),
}

/// A trace record plus the provenance needed to inspect its archive source.
#[derive(Debug, Clone)]
pub struct ArchivedTrace {
    /// Event `created_at` or recording `created_at`, used for range queries.
    pub recorded_at: DateTime<Utc>,
    /// Immutable archive file containing this record.
    pub archive_path: PathBuf,
    /// Decoded event or recording payload.
    pub record: ArchivedTraceRecord,
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

/// Enforce a finite compressed-byte cap on immutable trace archives.
///
/// Files are sorted by modification time and removed oldest-first until the
/// archive is at or below `max_bytes`. A zero cap is rejected so callers
/// cannot accidentally configure silent unlimited retention; use the finite
/// default or an explicit positive value instead.
pub fn enforce_trace_archive_size(cas_root: &Path, max_bytes: u64) -> Result<TraceArchiveEviction> {
    if max_bytes == 0 {
        return Err(crate::StoreError::Other(
            "trace archive size cap must be greater than zero".to_string(),
        ));
    }

    let archive_dir = cas_root.join(TRACE_ARCHIVE_DIR);
    let entries = match fs::read_dir(archive_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(TraceArchiveEviction::default());
        }
        Err(error) => return Err(error.into()),
    };

    let mut files = Vec::new();
    let mut total_bytes = 0u64;
    for entry in entries {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() || !is_trace_archive_name(&entry.file_name()) {
            continue;
        }
        let size = metadata.len();
        total_bytes = total_bytes.saturating_add(size);
        files.push((entry.path(), size, metadata.modified()?));
    }

    files.sort_by(|left, right| left.2.cmp(&right.2).then_with(|| left.0.cmp(&right.0)));

    let mut eviction = TraceArchiveEviction {
        remaining_bytes: total_bytes,
        ..TraceArchiveEviction::default()
    };
    for (path, size, _) in files {
        if eviction.remaining_bytes <= max_bytes {
            break;
        }

        fs::remove_file(&path)?;
        eviction.files_evicted += 1;
        eviction.bytes_evicted = eviction.bytes_evicted.saturating_add(size);
        eviction.remaining_bytes = eviction.remaining_bytes.saturating_sub(size);
        tracing::info!(
            path = %path.display(),
            bytes = size,
            cap_bytes = max_bytes,
            remaining_bytes = eviction.remaining_bytes,
            "evicted trace archive to enforce size cap"
        );
    }

    Ok(eviction)
}

fn validate_archive_range(from: DateTime<Utc>, to: DateTime<Utc>) -> Result<()> {
    if from > to {
        return Err(crate::StoreError::Other(
            "trace archive range start must not be after its end".to_string(),
        ));
    }
    Ok(())
}

fn read_jsonl_archive<T: DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let file = File::open(path)?;
    let decoder = zstd::stream::read::Decoder::new(file)?;
    let reader = BufReader::new(decoder);
    let mut records = Vec::new();
    for (line_number, line) in reader.lines().enumerate() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let record = serde_json::from_str(&line).map_err(|error| {
            crate::StoreError::Other(format!(
                "invalid trace archive record in {} at line {}: {error}",
                path.display(),
                line_number + 1
            ))
        })?;
        records.push(record);
    }
    Ok(records)
}

/// List archived event and recording traces in an inclusive time range.
///
/// Results are ordered oldest-first by their event/recording timestamp, with
/// archive path as a deterministic tie-breaker. The JSONL records are decoded
/// on demand, so callers can inspect the original payload without restoring
/// it to a mutable live table.
pub fn list_archived_traces(
    cas_root: &Path,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
) -> Result<Vec<ArchivedTrace>> {
    validate_archive_range(from, to)?;

    let archive_dir = cas_root.join(TRACE_ARCHIVE_DIR);
    let entries = match fs::read_dir(archive_dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };

    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if metadata.is_file() && is_trace_archive_name(&entry.file_name()) {
            paths.push(entry.path());
        }
    }
    paths.sort();

    let mut traces = Vec::new();
    for path in paths {
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.starts_with("events-") {
            for event in read_jsonl_archive::<Event>(&path)? {
                if (from..=to).contains(&event.created_at) {
                    traces.push(ArchivedTrace {
                        recorded_at: event.created_at,
                        archive_path: path.clone(),
                        record: ArchivedTraceRecord::Event(event),
                    });
                }
            }
        } else if name.starts_with("recordings-") {
            for recording in read_jsonl_archive::<RecordingArchive>(&path)? {
                if (from..=to).contains(&recording.recording.created_at) {
                    traces.push(ArchivedTrace {
                        recorded_at: recording.recording.created_at,
                        archive_path: path.clone(),
                        record: ArchivedTraceRecord::Recording(recording),
                    });
                }
            }
        }
    }

    traces.sort_by(|left, right| {
        left.recorded_at
            .cmp(&right.recorded_at)
            .then_with(|| left.archive_path.cmp(&right.archive_path))
    });
    Ok(traces)
}

/// Return a deterministic, evenly-spread sample of archived traces in a time
/// range. A zero `limit` returns no records; otherwise the first and last
/// matching records are retained when the range has more records than the
/// requested sample size.
pub fn sample_archived_traces(
    cas_root: &Path,
    from: DateTime<Utc>,
    to: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<ArchivedTrace>> {
    let traces = list_archived_traces(cas_root, from, to)?;
    if limit == 0 || traces.len() <= limit {
        return Ok(if limit == 0 { Vec::new() } else { traces });
    }
    if limit == 1 {
        return Ok(vec![traces[traces.len() / 2].clone()]);
    }

    let last = traces.len() - 1;
    let denominator = limit - 1;
    Ok((0..limit)
        .map(|index| traces[index * last / denominator].clone())
        .collect())
}

/// Remove archived trace files older than the configured retention period.
///
/// This compatibility helper is retained for callers of the original
/// cas-62a6 API. Daemon maintenance uses [`enforce_trace_archive_size`]
/// instead, so active retention is bounded by bytes rather than archive age.
/// A value of zero means keep forever. Retention is based on the immutable
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
    use cas_types::{Event, EventEntityType, EventType, Recording};
    use chrono::Duration;
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
        let old =
            write_jsonl_archive(&archive_dir, "events", &[serde_json::json!({"id": 1})]).unwrap();
        let recent =
            write_jsonl_archive(&archive_dir, "recordings", &[serde_json::json!({"id": 2})])
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

    #[test]
    fn size_cap_evicts_oldest_archive_files_and_reports_evictions() {
        let temp = TempDir::new().unwrap();
        let archive_dir = temp.path().join("archive");
        let oldest = write_jsonl_archive(
            &archive_dir,
            "events",
            &[serde_json::json!({"id": "oldest", "payload": "a"})],
        )
        .unwrap();
        let middle = write_jsonl_archive(
            &archive_dir,
            "events",
            &[serde_json::json!({"id": "middle", "payload": "b"})],
        )
        .unwrap();
        let newest = write_jsonl_archive(
            &archive_dir,
            "recordings",
            &[serde_json::json!({"id": "newest", "payload": "c"})],
        )
        .unwrap();

        let oldest_time = SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1);
        let middle_time = oldest_time + std::time::Duration::from_secs(1);
        let newest_time = middle_time + std::time::Duration::from_secs(1);
        File::open(&oldest)
            .unwrap()
            .set_modified(oldest_time)
            .unwrap();
        File::open(&middle)
            .unwrap()
            .set_modified(middle_time)
            .unwrap();
        File::open(&newest)
            .unwrap()
            .set_modified(newest_time)
            .unwrap();

        let oldest_size = fs::metadata(&oldest).unwrap().len();
        let cap = fs::metadata(&middle).unwrap().len() + fs::metadata(&newest).unwrap().len();
        let eviction = enforce_trace_archive_size(temp.path(), cap).unwrap();

        assert_eq!(eviction.files_evicted, 1);
        assert_eq!(eviction.bytes_evicted, oldest_size);
        assert!(!oldest.exists());
        assert!(middle.exists());
        assert!(newest.exists());
        assert_eq!(eviction.remaining_bytes, cap);
    }

    #[test]
    fn archived_traces_can_be_listed_and_sampled_by_time_range() {
        let temp = TempDir::new().unwrap();
        let archive_dir = temp.path().join("archive");
        let now = Utc::now();
        let first_time = now - Duration::days(3);
        let second_time = now - Duration::days(2);
        let third_time = now - Duration::days(1);

        let mut first = Event::new(
            EventType::TaskStarted,
            EventEntityType::Task,
            "task-first",
            "first",
        );
        first.created_at = first_time;
        let mut second = Event::new(
            EventType::TaskCompleted,
            EventEntityType::Task,
            "task-second",
            "second",
        );
        second.created_at = second_time;
        let mut third = Event::new(
            EventType::TaskBlocked,
            EventEntityType::Task,
            "task-third",
            "third",
        );
        third.created_at = third_time;

        write_jsonl_archive(&archive_dir, "events", &[first, second, third]).unwrap();

        let mut recording = Recording::new("/tmp/third.trace".to_string());
        recording.created_at = third_time;
        write_jsonl_archive(
            &archive_dir,
            "recordings",
            &[RecordingArchive {
                recording,
                agents: Vec::new(),
                events: Vec::new(),
                fts: Vec::new(),
            }],
        )
        .unwrap();

        let listed = list_archived_traces(temp.path(), second_time, third_time).unwrap();
        assert_eq!(listed.len(), 3);
        assert_eq!(listed[0].recorded_at, second_time);
        assert_eq!(listed[1].recorded_at, third_time);
        assert_eq!(listed[2].recorded_at, third_time);
        assert!(listed.iter().all(|trace| trace.archive_path.exists()));

        let sampled = sample_archived_traces(temp.path(), first_time, third_time, 2).unwrap();
        assert_eq!(sampled.len(), 2);
        assert_eq!(sampled[0].recorded_at, first_time);
        assert_eq!(sampled[1].recorded_at, third_time);
    }
}
