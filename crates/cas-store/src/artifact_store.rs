//! Durable records for published artifacts (cassy#910).
//!
//! One row per file a worker published. The row is written *before* any
//! network call, so a publish that cannot reach Cloud still leaves a
//! referenceable record — the message lane can cite `artifact_id` whether or
//! not storage is live.
//!
//! What this table deliberately never holds: the pre-signed upload URL and any
//! bearer token. Those are short-lived credentials; persisting one turns a
//! database read into an upload capability. `cloud_url` is the *durable*
//! location the server returns at complete, which is a different thing, and
//! [`SqliteArtifactStore::mark_uploaded`] refuses anything that looks like a
//! signed upload URL so the distinction cannot erode.

use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::error::StoreError;
use crate::{Result, shared_db};

/// SQLite DDL for the published-artifact ledger.
pub const ARTIFACT_SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS artifacts (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL,
    name TEXT NOT NULL,
    mime TEXT NOT NULL,
    size_bytes INTEGER NOT NULL,
    sha256 TEXT NOT NULL,
    cloud_artifact_id TEXT,
    status TEXT NOT NULL CHECK (status IN ('local', 'uploaded', 'committed')),
    cloud_url TEXT,
    slack_permalink TEXT,
    created_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_artifacts_task ON artifacts(task_id, created_at);
CREATE INDEX IF NOT EXISTS idx_artifacts_sha256 ON artifacts(sha256);
"#;

/// Statement-level form used by the numbered migration runner.
pub const ARTIFACT_SCHEMA_STATEMENTS: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS artifacts (
        id TEXT PRIMARY KEY,
        task_id TEXT NOT NULL,
        name TEXT NOT NULL,
        mime TEXT NOT NULL,
        size_bytes INTEGER NOT NULL,
        sha256 TEXT NOT NULL,
        cloud_artifact_id TEXT,
        status TEXT NOT NULL CHECK (status IN ('local', 'uploaded', 'committed')),
        cloud_url TEXT,
        slack_permalink TEXT,
        created_at TEXT NOT NULL
    )",
    "CREATE INDEX IF NOT EXISTS idx_artifacts_task ON artifacts(task_id, created_at)",
    "CREATE INDEX IF NOT EXISTS idx_artifacts_sha256 ON artifacts(sha256)",
];

/// One published artifact, as stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublishedArtifact {
    pub id: String,
    pub task_id: String,
    pub name: String,
    pub mime: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub cloud_artifact_id: Option<String>,
    pub status: String,
    /// Durable location the server returned at complete. Never an upload URL.
    pub cloud_url: Option<String>,
    pub slack_permalink: Option<String>,
    pub created_at: String,
}

/// What a caller supplies to record a freshly hashed local file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewArtifact {
    pub id: String,
    pub task_id: String,
    pub name: String,
    pub mime: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Query-string markers that identify a pre-signed (credential-bearing) URL.
///
/// The list covers the signing schemes the storage backends in play actually
/// emit — S3/R2 SigV4, GCS V4, and Azure SAS. A URL carrying any of them is a
/// capability, not a location, and must never reach the database.
const SIGNED_URL_MARKERS: &[&str] = &[
    "x-amz-signature",
    "x-amz-credential",
    "x-amz-security-token",
    "x-goog-signature",
    "x-goog-credential",
    "signature=",
    "sig=",
    "se=",
    "token=",
    "access_token=",
    "upload_id=",
];

/// True when `url` carries what looks like an embedded credential.
///
/// Conservative by construction: a false positive costs a persisted download
/// link, a false negative persists an upload capability.
pub fn looks_like_signed_upload_url(url: &str) -> bool {
    let lowered = url.to_ascii_lowercase();
    let query = lowered.split_once('?').map(|(_, rest)| rest);
    match query {
        Some(query) => SIGNED_URL_MARKERS
            .iter()
            .any(|marker| query.contains(marker)),
        None => false,
    }
}

/// SQLite store for the published-artifact ledger.
pub struct SqliteArtifactStore {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl SqliteArtifactStore {
    pub fn open(cas_dir: &Path) -> Result<Self> {
        let conn = shared_db::shared_connection(&cas_dir.join("cas.db"))?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    /// Build a store over an already-shared connection, so a caller that
    /// already holds the process connection does not open a second one.
    pub fn from_connection(conn: Arc<Mutex<rusqlite::Connection>>) -> Result<Self> {
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    pub fn init(&self) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        conn.execute_batch(ARTIFACT_SCHEMA)?;
        Ok(())
    }

    /// Record a hashed local file. The row lands as `local`; nothing about
    /// Cloud has happened yet.
    pub fn record_local(&self, artifact: &NewArtifact) -> Result<PublishedArtifact> {
        if artifact.id.trim().is_empty() {
            return Err(StoreError::Parse(
                "an artifact record requires an id".to_string(),
            ));
        }
        if artifact.task_id.trim().is_empty() {
            return Err(StoreError::Parse(
                "an artifact record requires a task id".to_string(),
            ));
        }
        if artifact.name.trim().is_empty() {
            return Err(StoreError::Parse(
                "an artifact record requires a name".to_string(),
            ));
        }
        if artifact.size_bytes == 0 {
            return Err(StoreError::Parse(
                "an artifact record requires a non-zero size".to_string(),
            ));
        }
        if artifact.sha256.len() != 64 {
            return Err(StoreError::Parse(format!(
                "an artifact digest must be 64 hex characters, got {}",
                artifact.sha256.len()
            )));
        }

        let created_at = Utc::now().to_rfc3339();
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        conn.execute(
            "INSERT INTO artifacts
                 (id, task_id, name, mime, size_bytes, sha256, cloud_artifact_id,
                  status, cloud_url, slack_permalink, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, 'local', NULL, NULL, ?7)",
            params![
                artifact.id,
                artifact.task_id,
                artifact.name,
                artifact.mime,
                artifact.size_bytes as i64,
                artifact.sha256,
                created_at,
            ],
        )?;

        Ok(PublishedArtifact {
            id: artifact.id.clone(),
            task_id: artifact.task_id.clone(),
            name: artifact.name.clone(),
            mime: artifact.mime.clone(),
            size_bytes: artifact.size_bytes,
            sha256: artifact.sha256.clone(),
            cloud_artifact_id: None,
            status: "local".to_string(),
            cloud_url: None,
            slack_permalink: None,
            created_at,
        })
    }

    /// Bytes reached the upload URL. Records the server's artifact id only —
    /// the upload URL itself is a credential and is never stored.
    pub fn mark_uploaded(&self, id: &str, cloud_artifact_id: &str) -> Result<()> {
        if cloud_artifact_id.trim().is_empty() {
            return Err(StoreError::Parse(
                "an uploaded artifact requires the cloud artifact id".to_string(),
            ));
        }
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let changed = conn.execute(
            "UPDATE artifacts SET cloud_artifact_id = ?2, status = 'uploaded'
             WHERE id = ?1 AND status = 'local'",
            params![id, cloud_artifact_id],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!(
                "no local artifact {id} to mark uploaded"
            )));
        }
        Ok(())
    }

    /// The server confirmed the object. `cloud_url` is the durable location;
    /// a signed upload URL is refused here rather than quietly stored.
    pub fn mark_committed(&self, id: &str, cloud_url: Option<&str>) -> Result<()> {
        if let Some(url) = cloud_url
            && looks_like_signed_upload_url(url)
        {
            return Err(StoreError::Parse(
                "refusing to persist a signed upload URL as an artifact location".to_string(),
            ));
        }
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let changed = conn.execute(
            "UPDATE artifacts SET status = 'committed', cloud_url = ?2
             WHERE id = ?1 AND status IN ('local', 'uploaded')",
            params![id, cloud_url],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!(
                "no pending artifact {id} to mark committed"
            )));
        }
        Ok(())
    }

    /// Attach the Slack permalink a later relay produced.
    pub fn set_slack_permalink(&self, id: &str, permalink: &str) -> Result<()> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let changed = conn.execute(
            "UPDATE artifacts SET slack_permalink = ?2 WHERE id = ?1",
            params![id, permalink],
        )?;
        if changed == 0 {
            return Err(StoreError::NotFound(format!("no artifact {id}")));
        }
        Ok(())
    }

    pub fn get(&self, id: &str) -> Result<Option<PublishedArtifact>> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let mut stmt = conn.prepare_cached(
            "SELECT id, task_id, name, mime, size_bytes, sha256, cloud_artifact_id,
                    status, cloud_url, slack_permalink, created_at
             FROM artifacts WHERE id = ?1",
        )?;
        let row = stmt.query_row([id], row_to_artifact).optional()?;
        Ok(row)
    }

    /// Newest first, so `cas artifact list` leads with what was just published.
    pub fn list_for_task(&self, task_id: &str) -> Result<Vec<PublishedArtifact>> {
        let conn = self.conn.lock().unwrap_or_else(|p| p.into_inner());
        let mut stmt = conn.prepare_cached(
            "SELECT id, task_id, name, mime, size_bytes, sha256, cloud_artifact_id,
                    status, cloud_url, slack_permalink, created_at
             FROM artifacts WHERE task_id = ?1 ORDER BY created_at DESC, id DESC",
        )?;
        let rows = stmt
            .query_map([task_id], row_to_artifact)?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(rows)
    }
}

fn row_to_artifact(row: &rusqlite::Row<'_>) -> rusqlite::Result<PublishedArtifact> {
    Ok(PublishedArtifact {
        id: row.get(0)?,
        task_id: row.get(1)?,
        name: row.get(2)?,
        mime: row.get(3)?,
        size_bytes: row.get::<_, i64>(4)?.max(0) as u64,
        sha256: row.get(5)?,
        cloud_artifact_id: row.get(6)?,
        status: row.get(7)?,
        cloud_url: row.get(8)?,
        slack_permalink: row.get(9)?,
        created_at: row.get(10)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn store() -> (TempDir, SqliteArtifactStore) {
        let dir = TempDir::new().unwrap();
        let store = SqliteArtifactStore::open(dir.path()).unwrap();
        (dir, store)
    }

    fn new_artifact(id: &str) -> NewArtifact {
        NewArtifact {
            id: id.to_string(),
            task_id: "cas-b72a".to_string(),
            name: "brief.pdf".to_string(),
            mime: "application/pdf".to_string(),
            size_bytes: 4_096,
            sha256: "b".repeat(64),
        }
    }

    #[test]
    fn a_recorded_artifact_starts_local_and_reads_back() {
        let (_dir, store) = store();
        let recorded = store.record_local(&new_artifact("art-1")).unwrap();
        assert_eq!(recorded.status, "local");
        assert!(recorded.cloud_artifact_id.is_none());

        let fetched = store.get("art-1").unwrap().expect("row must exist");
        assert_eq!(fetched, recorded);
        assert!(store.get("art-missing").unwrap().is_none());
    }

    #[test]
    fn the_lifecycle_moves_local_to_uploaded_to_committed() {
        let (_dir, store) = store();
        store.record_local(&new_artifact("art-1")).unwrap();

        store.mark_uploaded("art-1", "cloud-99").unwrap();
        let uploaded = store.get("art-1").unwrap().unwrap();
        assert_eq!(uploaded.status, "uploaded");
        assert_eq!(uploaded.cloud_artifact_id.as_deref(), Some("cloud-99"));

        store
            .mark_committed("art-1", Some("https://cloud.example/a/cloud-99"))
            .unwrap();
        let committed = store.get("art-1").unwrap().unwrap();
        assert_eq!(committed.status, "committed");
        assert_eq!(
            committed.cloud_url.as_deref(),
            Some("https://cloud.example/a/cloud-99")
        );
    }

    #[test]
    fn a_row_never_moves_backwards() {
        let (_dir, store) = store();
        store.record_local(&new_artifact("art-1")).unwrap();
        store.mark_uploaded("art-1", "cloud-99").unwrap();

        let error = store.mark_uploaded("art-1", "cloud-99").unwrap_err();
        assert!(
            error.to_string().contains("no local artifact"),
            "a second upload of the same row must not be silently accepted: {error}"
        );

        store.mark_committed("art-1", None).unwrap();
        assert!(
            store.mark_committed("art-1", None).is_err(),
            "a committed row is terminal"
        );
    }

    #[test]
    fn a_signed_upload_url_is_refused_as_a_location() {
        let (_dir, store) = store();
        store.record_local(&new_artifact("art-1")).unwrap();

        for signed in [
            "https://bucket.s3.amazonaws.com/o?X-Amz-Signature=deadbeef&X-Amz-Expires=900",
            "https://storage.googleapis.com/o?X-Goog-Signature=abc",
            "https://acct.blob.core.windows.net/c/o?sig=abc&se=2026-09-19T00%3A00%3A00Z",
            "https://cloud.example/upload?token=secret-bearer",
        ] {
            assert!(
                looks_like_signed_upload_url(signed),
                "must classify as signed: {signed}"
            );
            let error = store.mark_committed("art-1", Some(signed)).unwrap_err();
            assert!(error.to_string().contains("refusing to persist"), "{error}");
        }

        assert!(
            !looks_like_signed_upload_url("https://cloud.example/a/cloud-99"),
            "a plain durable location is not a credential"
        );
        assert!(
            !looks_like_signed_upload_url("https://cloud.example/a/cloud-99?download=1"),
            "an innocuous query must not trip the guard"
        );
        // The row is untouched by the refusals.
        assert_eq!(store.get("art-1").unwrap().unwrap().status, "local");
    }

    #[test]
    fn listing_is_scoped_to_a_task_and_newest_first() {
        let (_dir, store) = store();
        let mut first = new_artifact("art-1");
        first.name = "first.pdf".to_string();
        store.record_local(&first).unwrap();
        let mut second = new_artifact("art-2");
        second.name = "second.pdf".to_string();
        store.record_local(&second).unwrap();
        let mut other = new_artifact("art-3");
        other.task_id = "cas-other".to_string();
        store.record_local(&other).unwrap();

        let listed = store.list_for_task("cas-b72a").unwrap();
        assert_eq!(listed.len(), 2, "the other task's row must not appear");
        assert_eq!(listed[0].id, "art-2", "newest first");
        assert_eq!(listed[1].id, "art-1");
    }

    #[test]
    fn a_malformed_record_is_refused_before_it_reaches_the_table() {
        let (_dir, store) = store();
        for mutate in [
            (|a: &mut NewArtifact| a.id = String::new()) as fn(&mut NewArtifact),
            |a: &mut NewArtifact| a.task_id = String::new(),
            |a: &mut NewArtifact| a.name = "  ".to_string(),
            |a: &mut NewArtifact| a.size_bytes = 0,
            |a: &mut NewArtifact| a.sha256 = "short".to_string(),
        ] {
            let mut artifact = new_artifact("art-x");
            mutate(&mut artifact);
            assert!(store.record_local(&artifact).is_err());
        }
        assert!(
            store.list_for_task("cas-b72a").unwrap().is_empty(),
            "no partial row may survive a rejected record"
        );
    }

    #[test]
    fn the_status_column_rejects_a_value_outside_the_lifecycle() {
        let (_dir, store) = store();
        store.record_local(&new_artifact("art-1")).unwrap();
        let conn = store.conn.lock().unwrap();
        let error = conn
            .execute(
                "UPDATE artifacts SET status = 'published' WHERE id = 'art-1'",
                [],
            )
            .unwrap_err();
        assert!(
            error.to_string().contains("CHECK"),
            "the schema, not just the API, pins the lifecycle: {error}"
        );
    }
}
