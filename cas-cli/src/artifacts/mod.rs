//! Publishing a local file as a durable, referenceable artifact (cassy#910).
//!
//! The lane a worker sees is one call with a local path. Everything the
//! harness would otherwise have to get right — where the file may live, how
//! big it may be, what its digest is, whether Cloud storage exists — happens
//! here, in this order:
//!
//! 1. Resolve the path against the task's publishable roots ([`paths`]).
//! 2. Hash and measure the bytes, and refuse anything over the ceiling
//!    **before** a single network call is made.
//! 3. Record the row as `local`. It is referenceable from this moment,
//!    whether or not the upload that follows succeeds.
//! 4. Try Cloud, if this installation has credentials. A missing endpoint
//!    leaves the row `local` and is reported as a boundary, not an error.
//!
//! The order matters: a publish that cannot reach Cloud must still leave the
//! operator with something to cite.
//!
//! Once an artifact is committed, [`signed_view`] trades its record id for a
//! short-lived signed view URL, which is how Commander opens a report card,
//! and [`slack`] posts it to Slack through Violet by that same id.

pub mod cloud;
pub mod paths;
pub mod proof;
pub(crate) mod qa;
pub mod retention;
pub mod slack;

use cas_store::{NewArtifact, PublishedArtifact, SqliteArtifactStore};
use cas_types::{ARTIFACT_MAX_BYTES, ArtifactRef};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use cloud::{
    ArtifactDigest, ArtifactUploadClient, BeginRequest, UploadFailure, ViewFailure, ViewUrlResponse,
};
use paths::{PublishRoots, resolve_publishable_path};

pub use proof::{MetadataUnavailable, lookup_metadata};

/// 64 KiB: large enough that hashing a 25 MiB file is a handful of syscalls,
/// small enough not to matter on a constrained host.
const HASH_CHUNK: usize = 64 * 1024;

/// What a publish did, in the operator's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishDisposition {
    /// Recorded locally and committed to Cloud storage.
    Committed { cloud_url: Option<String> },
    /// Cloud has the bytes, but its portable task reference was not saved.
    CommittedMetadataUnavailable { reason: String },
    /// The manifest was recorded, but its QA evidence cannot travel to a peer.
    QaBundleNotPortable {
        reason: String,
        storage: Box<PublishDisposition>,
    },
    /// Recorded locally; Cloud storage is not live on this endpoint yet.
    StorageNotLive { reason: String },
    /// Recorded locally; this installation has no cloud credentials.
    NotLoggedIn,
    /// Recorded locally; the upload was attempted and did not complete. The
    /// record is still usable, and the reason says what to do.
    UploadFailed { reason: String },
}

/// Storage state without a QA warning wrapper. Renderers can match this
/// exhaustively without assuming a recursive warning has been unwrapped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StorageDisposition<'a> {
    Committed,
    CommittedMetadataUnavailable { reason: &'a str },
    StorageNotLive { reason: &'a str },
    NotLoggedIn,
    UploadFailed { reason: &'a str },
}

impl PublishDisposition {
    /// Whether Cloud committed the bytes. Portable metadata can fail separately.
    pub fn is_committed(&self) -> bool {
        matches!(
            self.storage_disposition(),
            StorageDisposition::Committed
                | StorageDisposition::CommittedMetadataUnavailable { .. }
        )
    }

    pub fn metadata_state(&self) -> &'static str {
        match self {
            Self::QaBundleNotPortable { .. } => "qa_bundle_not_portable",
            Self::Committed { .. } => "task_reference_recorded",
            Self::CommittedMetadataUnavailable { .. } => "metadata_unavailable",
            _ => "local_only",
        }
    }

    pub fn qa_portability_error(&self) -> Option<&str> {
        match self {
            Self::QaBundleNotPortable { reason, .. } => Some(reason),
            _ => None,
        }
    }

    pub(crate) fn storage_disposition(&self) -> StorageDisposition<'_> {
        match self {
            Self::QaBundleNotPortable { storage, .. } => storage.storage_disposition(),
            Self::Committed { .. } => StorageDisposition::Committed,
            Self::CommittedMetadataUnavailable { reason } => StorageDisposition::CommittedMetadataUnavailable { reason },
            Self::StorageNotLive { reason } => StorageDisposition::StorageNotLive { reason },
            Self::NotLoggedIn => StorageDisposition::NotLoggedIn,
            Self::UploadFailed { reason } => StorageDisposition::UploadFailed { reason },
        }
    }

    fn with_qa_error(self, error: Option<String>) -> Self {
        match error {
            Some(reason) => Self::QaBundleNotPortable {
                reason,
                storage: Box::new(self),
            },
            None => self,
        }
    }

    /// One line for a person, naming what happened and what follows from it.
    pub fn summary(&self) -> String {
        match self {
            PublishDisposition::QaBundleNotPortable { reason, storage } => {
                format!("{}; QA bundle not portable: {reason}", storage.summary())
            }
            PublishDisposition::Committed {
                cloud_url: Some(url),
            } => {
                format!("stored in Cloud at {url}")
            }
            PublishDisposition::Committed { cloud_url: None } => {
                "stored in Cloud (private)".to_string()
            }
            PublishDisposition::CommittedMetadataUnavailable { .. } => {
                "stored in Cloud; peer metadata unavailable".to_string()
            }
            PublishDisposition::StorageNotLive { .. } => {
                "recorded locally; Cloud storage is not live yet".to_string()
            }
            PublishDisposition::NotLoggedIn => {
                "recorded locally; not logged in to Cassy Cloud".to_string()
            }
            PublishDisposition::UploadFailed { .. } => {
                "recorded locally; Cloud delivery failed".to_string()
            }
        }
    }
}

/// The result of one publish.
#[derive(Debug, Clone)]
pub struct PublishOutcome {
    pub artifact: PublishedArtifact,
    pub disposition: PublishDisposition,
    /// The canonical path the bytes were read from.
    pub source: PathBuf,
}

impl PublishOutcome {
    /// The portable half, for a message lane or a task note.
    pub fn artifact_ref(&self) -> ArtifactRef {
        ArtifactRef {
            artifact_id: self.artifact.id.clone(),
            name: self.artifact.name.clone(),
            mime: self.artifact.mime.clone(),
            size_bytes: self.artifact.size_bytes,
            sha256: self.artifact.sha256.clone(),
        }
    }
}

/// Why a publish could not happen at all. Distinct from a storage outcome:
/// these leave no record.
#[derive(Debug)]
pub enum PublishError {
    Path(paths::PathRefusal),
    TooLarge { size_bytes: u64, path: String },
    Read(String),
    Store(String),
}

impl std::fmt::Display for PublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PublishError::Path(refusal) => write!(f, "{refusal}"),
            PublishError::TooLarge { size_bytes, path } => write!(
                f,
                "{path} is {size_bytes} bytes; the publishable ceiling is {ARTIFACT_MAX_BYTES} \
                 ({} MiB). Nothing was uploaded.",
                ARTIFACT_MAX_BYTES / (1024 * 1024)
            ),
            PublishError::Read(detail) => write!(f, "{detail}"),
            PublishError::Store(detail) => write!(f, "could not record the artifact: {detail}"),
        }
    }
}

impl std::error::Error for PublishError {}

/// Where the bytes may come from and where the record goes.
pub struct PublishContext<'a> {
    pub store: &'a SqliteArtifactStore,
    pub roots: PublishRoots,
    pub task_id: String,
    /// `None` when this installation has no cloud credentials.
    pub cloud: Option<ArtifactUploadClient>,
}

/// The artifact client for the project at `cas_root`, or `None` when this
/// installation has no Cloud credentials (a supported state, not an error).
/// It names the project's active team and canonical project id, so Cloud can
/// place an artifact whose task has not been pushed yet.
pub fn cloud_client(cas_root: &Path) -> Option<ArtifactUploadClient> {
    let config =
        crate::cloud::CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root).ok()?;
    if !config.is_logged_in() {
        return None;
    }
    let token = config.token.clone()?;
    Some(
        ArtifactUploadClient::new(&config.endpoint, &token).with_scope(
            config.active_team_id(),
            crate::cloud::resolve_canonical_id_for_sync(cas_root).ok(),
        ),
    )
}

/// A committed artifact and a short-lived signed URL to view it.
#[derive(Debug, Clone)]
pub struct SignedView {
    pub artifact: PublishedArtifact,
    pub view: ViewUrlResponse,
}

/// Why an artifact cannot be viewed through Cloud.
#[derive(Debug)]
pub enum ViewError {
    /// No record by that id in this project.
    Unknown(String),
    /// The record exists but never committed to Cloud; its bytes are only on
    /// the machine that published it.
    NotInCloud {
        status: String,
    },
    /// This installation has no Cloud credentials.
    NotLoggedIn,
    /// Cloud refused or failed.
    Cloud(ViewFailure),
    Store(String),
}

impl std::fmt::Display for ViewError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ViewError::Unknown(id) => write!(f, "no artifact {id} in this project"),
            ViewError::NotInCloud { status } => write!(
                f,
                "this artifact is {status}: it was never stored in Cloud, so only the machine \
                 that published it has the file"
            ),
            ViewError::NotLoggedIn => f.write_str("not logged in to Cassy Cloud"),
            ViewError::Cloud(failure) => write!(f, "{failure}"),
            ViewError::Store(detail) => write!(f, "could not read the artifact record: {detail}"),
        }
    }
}

impl std::error::Error for ViewError {}

/// A signed view URL for the committed artifact `id` (a local record id,
/// `art-…`). Only a record Cloud committed has one; a `local` or `uploaded`
/// record answers [`ViewError::NotInCloud`] without a network call.
pub fn signed_view(
    store: &SqliteArtifactStore,
    client: Option<&ArtifactUploadClient>,
    id: &str,
) -> Result<SignedView, ViewError> {
    let artifact = store
        .get(id.trim())
        .map_err(|error| ViewError::Store(error.to_string()))?
        .ok_or_else(|| ViewError::Unknown(id.trim().to_string()))?;
    let cloud_id = match (
        artifact.status.as_str(),
        artifact.cloud_artifact_id.as_deref(),
    ) {
        ("committed", Some(cloud_id)) if !cloud_id.trim().is_empty() => cloud_id.to_string(),
        _ => {
            return Err(ViewError::NotInCloud {
                status: artifact.status.clone(),
            });
        }
    };
    let client = client.ok_or(ViewError::NotLoggedIn)?;
    let view = client.view_url(&cloud_id).map_err(ViewError::Cloud)?;
    Ok(SignedView { artifact, view })
}

/// Hash and measure a file without holding it in memory.
///
/// Returns `(sha256_hex, size_bytes)`. The ceiling is checked by the caller so
/// the refusal can name the path; this function reports what it measured.
pub fn digest_file(path: &Path) -> Result<(String, u64), PublishError> {
    let file = File::open(path)
        .map_err(|error| PublishError::Read(format!("cannot read {}: {error}", path.display())))?;
    let mut reader = BufReader::with_capacity(HASH_CHUNK, file);
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK];
    let mut size_bytes: u64 = 0;
    loop {
        let read = reader.read(&mut buffer).map_err(|error| {
            PublishError::Read(format!("cannot read {}: {error}", path.display()))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size_bytes += read as u64;
    }
    Ok((format!("{:x}", hasher.finalize()), size_bytes))
}

/// Media type from the file extension. Ambiguous TypeScript extensions use
/// text unless their bytes have MPEG-TS framing. Unknown extensions get the
/// honest `application/octet-stream` rather than a guess.
pub fn mime_for(path: &Path) -> String {
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "pdf" => "application/pdf",
        "html" | "htm" => "text/html",
        "md" | "markdown" => "text/markdown",
        "txt" | "log" => "text/plain",
        "ts" | "tsx" => {
            // A lone 0x47 is also ASCII G in valid TypeScript. Require two
            // complete 188-byte transport packets with repeated sync bytes.
            const PACKET_BYTES: usize = 188;
            let mut prefix = [0u8; PACKET_BYTES * 2];
            let framed = File::open(path)
                .and_then(|mut file| file.read_exact(&mut prefix))
                .is_ok()
                && prefix[0] == 0x47
                && prefix[PACKET_BYTES] == 0x47;
            if framed { "video/mp2t" } else { "text/plain" }
        }
        "json" => "application/json",
        "jsonl" | "ndjson" => "application/x-ndjson",
        "csv" => "text/csv",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "webm" => "video/webm",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "xz" => "application/x-xz",
        "yaml" | "yml" => "application/yaml",
        "toml" => "text/plain",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Mint a local record id. Random rather than content-derived, so publishing
/// the same bytes twice produces two citable records instead of a collision.
fn mint_artifact_id() -> String {
    let uuid = uuid::Uuid::new_v4();
    format!("art-{}", &uuid.simple().to_string()[..8])
}

/// Publish one file: validate, hash, record, then try Cloud.
pub fn publish(
    context: &PublishContext<'_>,
    candidate: &Path,
) -> Result<PublishOutcome, PublishError> {
    let source = resolve_publishable_path(candidate, &context.roots).map_err(PublishError::Path)?;

    let (sha256, size_bytes) = digest_file(&source)?;
    if size_bytes == 0 {
        return Err(PublishError::Read(format!(
            "{} is empty; there is nothing to publish",
            source.display()
        )));
    }
    // The ceiling is enforced here, before the client is even consulted, so a
    // too-large file never opens a connection.
    if size_bytes > ARTIFACT_MAX_BYTES {
        return Err(PublishError::TooLarge {
            size_bytes,
            path: source.display().to_string(),
        });
    }

    // Portability is optional: a missing/invalid companion must never prevent
    // publishing the requested manifest through the ordinary artifact path.
    let (closure, mut qa_error) = match qa::prepare(context, &source) {
        Ok(closure) => (closure, None),
        Err(error) => {
            tracing::warn!(path = %source.display(), %error, "QA companion archive unavailable; publishing manifest only");
            (None, Some(error.to_string()))
        }
    };

    let name = source
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("artifact")
        .to_string();
    let record = NewArtifact {
        id: mint_artifact_id(),
        task_id: context.task_id.clone(),
        name,
        mime: mime_for(&source),
        size_bytes,
        sha256,
    };
    let artifact = context
        .store
        .record_local(&record)
        .map_err(|error| PublishError::Store(error.to_string()))?;

    let Some(client) = context.cloud.as_ref() else {
        return Ok(PublishOutcome {
            artifact,
            disposition: PublishDisposition::NotLoggedIn.with_qa_error(qa_error),
            source,
        });
    };

    let mut disposition = match upload(client, context.store, &artifact, &source) {
        Ok(cloud_url) => PublishDisposition::Committed { cloud_url },
        Err(UploadFailure::NotLive { reason }) => PublishDisposition::StorageNotLive { reason },
        Err(other) => PublishDisposition::UploadFailed {
            reason: other.to_string(),
        },
    };

    // Re-read so the caller reports the row as stored, not as assembled.
    let artifact = context
        .store
        .get(&artifact.id)
        .map_err(|error| PublishError::Store(error.to_string()))?
        .unwrap_or(artifact);

    if disposition.is_committed() {
        if let Err(error) =
            proof::record_published(&context.roots.project_root.join(".cas"), &artifact)
        {
            disposition = PublishDisposition::CommittedMetadataUnavailable {
                reason: error.to_string(),
            };
        }
    }

    let mut outcome = PublishOutcome {
        artifact,
        disposition,
        source,
    };
    if let Some((_temp, archive)) = closure {
        if outcome.disposition.is_committed() {
            if let Err(error) = qa::record(context, &outcome, &archive) {
                qa_error = Some(error.to_string());
            }
        } else {
            qa_error = Some("manifest is not stored in Cloud, so its QA archive is not portable".into());
        }
    }
    outcome.disposition = outcome.disposition.with_qa_error(qa_error);
    Ok(outcome)
}

/// The three cloud steps, advancing the row as each one lands.
fn upload(
    client: &ArtifactUploadClient,
    store: &SqliteArtifactStore,
    artifact: &PublishedArtifact,
    source: &Path,
) -> Result<Option<String>, UploadFailure> {
    let begun = client.begin(&BeginRequest {
        task_id: artifact.task_id.clone(),
        name: artifact.name.clone(),
        mime: artifact.mime.clone(),
        size_bytes: artifact.size_bytes,
        sha256: artifact.sha256.clone(),
        // The client supplies its team and project scope.
        team_id: None,
        project_id: None,
    })?;

    let file = File::open(source).map_err(|error| UploadFailure::Failed {
        step: cloud::UploadStep::Upload,
        reason: format!("cannot reopen {} to upload: {error}", source.display()),
        interaction: format!("open {}", source.display()),
    })?;
    client.upload(
        &begun,
        &artifact.mime,
        artifact.size_bytes,
        BufReader::with_capacity(HASH_CHUNK, file),
    )?;

    // Record the server's id before asking it to commit: if `complete` fails,
    // the row still names the object that exists on the far side.
    if let Err(error) = store.mark_uploaded(&artifact.id, &begun.artifact_id) {
        return Err(UploadFailure::Failed {
            step: cloud::UploadStep::Upload,
            reason: format!("could not record the upload locally: {error}"),
            interaction: format!("artifact {}", artifact.id),
        });
    }

    let completed = client.complete(
        &begun.artifact_id,
        &ArtifactDigest {
            sha256: Some(artifact.sha256.clone()),
            size_bytes: Some(artifact.size_bytes),
        },
    )?;

    // Retry only local bookkeeping: replaying Cloud or proof attachment could
    // create another object or receipt. The store retry honors wait budgets.
    if let Err(error) = cas_store::shared_db::with_write_retry(|| {
        store.mark_committed(&artifact.id, completed.url.as_deref())
    }) {
        return Err(UploadFailure::Failed {
            step: cloud::UploadStep::Complete,
            reason: format!("uploaded, not committed locally: {error}"),
            interaction: format!("artifact {}", artifact.id),
        });
    }
    Ok(completed.url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use tempfile::TempDir;

    struct Fixture {
        _dir: TempDir,
        store: SqliteArtifactStore,
        roots: PublishRoots,
        task_dir: PathBuf,
        project_root: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = TempDir::new().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let project_root = base.join("project");
        let cas_root = project_root.join(".cas");
        let artifacts_root = base.join("artifacts");
        let task_dir = crate::config::project_factory_artifacts_root(&cas_root, &artifacts_root)
            .join("cas-b72a");
        fs::create_dir_all(&cas_root).unwrap();
        fs::create_dir_all(&task_dir).unwrap();
        let store = SqliteArtifactStore::open(&cas_root).unwrap();
        let roots = PublishRoots::new(&cas_root, &artifacts_root, "cas-b72a");
        Fixture {
            _dir: dir,
            store,
            roots,
            task_dir,
            project_root,
        }
    }

    impl Fixture {
        fn context(&self) -> PublishContext<'_> {
            PublishContext {
                store: &self.store,
                roots: self.roots.clone(),
                task_id: "cas-b72a".to_string(),
                cloud: None,
            }
        }
    }

    #[test]
    fn a_published_file_records_the_digest_and_size_of_its_bytes() {
        let f = fixture();
        let file = f.task_dir.join("brief.pdf");
        fs::write(&file, b"hello world").unwrap();

        let outcome = publish(&f.context(), &file).unwrap();
        assert_eq!(outcome.artifact.size_bytes, 11);
        assert_eq!(
            outcome.artifact.sha256,
            // sha256("hello world")
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
        assert_eq!(outcome.artifact.mime, "application/pdf");
        assert_eq!(outcome.artifact.name, "brief.pdf");
        assert_eq!(outcome.artifact.status, "local");
        assert_eq!(outcome.disposition, PublishDisposition::NotLoggedIn);
        assert!(outcome.artifact.id.starts_with("art-"));

        let reference = outcome.artifact_ref();
        reference.validate().expect("the minted ref must be valid");
        assert_eq!(reference.artifact_id, outcome.artifact.id);
    }

    #[test]
    fn the_digest_matches_a_streaming_hash_of_a_multi_chunk_file() {
        let f = fixture();
        let file = f.task_dir.join("big.bin");
        // Deliberately larger than one hash chunk, so a single-read
        // implementation would disagree.
        let bytes: Vec<u8> = (0..(HASH_CHUNK * 3 + 17))
            .map(|i| (i % 251) as u8)
            .collect();
        fs::write(&file, &bytes).unwrap();

        let (sha256, size) = digest_file(&file).unwrap();
        assert_eq!(size, bytes.len() as u64);
        assert_eq!(sha256, format!("{:x}", Sha256::digest(&bytes)));
    }

    #[test]
    fn a_file_over_the_ceiling_is_refused_and_leaves_no_record() {
        let f = fixture();
        let file = f.task_dir.join("huge.bin");
        fs::write(&file, vec![0u8; (ARTIFACT_MAX_BYTES + 1) as usize]).unwrap();

        let error = publish(&f.context(), &file).unwrap_err();
        assert!(
            matches!(error, PublishError::TooLarge { .. }),
            "got {error:?}"
        );
        assert!(
            error.to_string().contains("Nothing was uploaded"),
            "{error}"
        );
        assert!(
            f.store.list_for_task("cas-b72a").unwrap().is_empty(),
            "a refused publish must not leave a row"
        );
    }

    #[test]
    fn a_file_exactly_at_the_ceiling_is_accepted() {
        let f = fixture();
        let file = f.task_dir.join("at-limit.bin");
        fs::write(&file, vec![7u8; ARTIFACT_MAX_BYTES as usize]).unwrap();
        let outcome = publish(&f.context(), &file).unwrap();
        assert_eq!(outcome.artifact.size_bytes, ARTIFACT_MAX_BYTES);
    }

    #[test]
    fn an_empty_file_is_refused() {
        let f = fixture();
        let file = f.task_dir.join("empty.pdf");
        fs::write(&file, b"").unwrap();
        let error = publish(&f.context(), &file).unwrap_err();
        assert!(error.to_string().contains("nothing to publish"), "{error}");
    }

    #[tokio::test]
    async fn producer_publication_keeps_manifest_when_closure_is_unavailable() {
        use wiremock::matchers::{body_string_contains, method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/artifacts/begin"))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "artifact_id": "cloud-manifest", "upload_url": format!("{}/blob", server.uri()),
                "required_headers": {}
            })))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/blob"))
            .respond_with(ResponseTemplate::new(200))
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/blob"))
            .and(body_string_contains("trace.zip"))
            .respond_with(ResponseTemplate::new(403))
            .with_priority(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/artifacts/cloud-manifest/complete"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({"url": null})),
            )
            .mount(&server)
            .await;
        let endpoint = server.uri();
        tokio::task::spawn_blocking(move || {
            let f = fixture();
            crate::store::open_task_store(&f.project_root.join(".cas"))
                .unwrap()
                .add(&crate::types::Task::new(
                    "cas-b72a".into(),
                    "artifact".into(),
                ))
                .unwrap();
            let manifest = f.task_dir.join("bundle.json");
            let private = f.project_root.join("private.txt");
            fs::write(&private, "private sentinel").unwrap();
            let private_bytes = fs::read(&private).unwrap();
            fs::File::create(f.task_dir.join("oversized.zip"))
                .unwrap()
                .set_len(ARTIFACT_MAX_BYTES + 1)
                .unwrap();
            for online in [true, false] {
                let mut context = f.context();
                if online {
                    context.cloud = Some(ArtifactUploadClient::new(&endpoint, "test-token"));
                }
                for path in [
                    "oversized.zip".into(),
                    "../private.txt".to_string(),
                    private.display().to_string(),
                ] {
                    fs::write(
                        &manifest,
                        serde_json::json!({
                            "schema": 1, "task_id": "cas-b72a", "producer": "cas-qa-craft",
                            "files": {"trace": path}
                        })
                        .to_string(),
                    )
                    .unwrap();
                    assert!(qa::prepare(&context, &manifest).is_err());
                    let published = publish(&context, &manifest).unwrap();
                    assert_eq!(published.artifact.name, "bundle.json");
                    assert_eq!(
                        published.artifact.status,
                        if online { "committed" } else { "local" }
                    );
                    assert!(
                        published.disposition.summary().contains("not portable"),
                        "{published:?}"
                    );
                    if path == "oversized.zip" {
                        assert!(
                            published.disposition.summary().contains("byte limit"),
                            "{published:?}"
                        );
                    }
                }
            }
            // A valid archive cannot travel either if its manifest upload fails.
            fs::write(f.task_dir.join("trace.zip"), b"saved QA trace").unwrap();
            fs::write(&manifest, serde_json::json!({
                "schema": 1, "task_id": "cas-b72a", "producer": "cas-qa-craft",
                "files": {"trace": "trace.zip"}
            }).to_string()).unwrap();
            let mut context = f.context();
            context.cloud = Some(ArtifactUploadClient::new(&endpoint, "test-token"));
            assert!(qa::prepare(&context, &manifest).unwrap().is_some());
            let published = publish(&context, &manifest).unwrap();
            assert_eq!(published.artifact.status, "local");
            assert_eq!(published.disposition.metadata_state(), "qa_bundle_not_portable");
            assert!(published.disposition.qa_portability_error().unwrap().contains("manifest"));
            assert_eq!(fs::read(private).unwrap(), private_bytes);
        })
        .await
        .unwrap();
    }

    #[test]
    fn real_qa_craft_parts_manifest_exports_only_declared_files() {
        let f = fixture();
        // Original closed cas-abb7 bundle, retained verbatim as regression
        // input. Only the task binding changes for this isolated store.
        let mut manifest: serde_json::Value =
            serde_json::from_str(include_str!("../../tests/fixtures/qa-parts-bundle.json"))
                .unwrap();
        manifest["task_id"] = "cas-b72a".into();
        let dir = f.task_dir.join("qa");
        fs::create_dir_all(&dir).unwrap();
        let files = manifest["files"].clone();
        let mut declared = Vec::new();
        for key in [
            "trace",
            "trace_actions",
            "receipt",
            "aria_yaml",
            "aria_json",
            "visual_qa",
            "visual_qa_json",
            "visual_qa_stdout",
            "critique",
        ] {
            declared.push(files[key].as_str().unwrap());
        }
        for key in ["cells", "a11y", "polish_screenshots"] {
            declared.extend(
                files[key]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|path| path.as_str().unwrap()),
            );
        }
        let expected: BTreeSet<_> = declared
            .iter()
            .map(|path| PathBuf::from("qa").join(path))
            .chain([PathBuf::from("qa/bundle.json")])
            .collect();
        for part in files["parts"].as_array().unwrap() {
            for key in ["receipt", "aria_yaml", "aria_json"] {
                declared.push(part[key].as_str().unwrap());
            }
        }
        for part in manifest["files"]["parts"].as_array_mut().unwrap() {
            part["trace"] = "parts/unread-trace.zip".into();
            part["trace_actions"] = "parts/unread-actions.txt".into();
        }
        declared.extend(["parts/unread-trace.zip", "parts/unread-actions.txt"]);
        for path in declared {
            let path = dir.join(path);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"fixture capture").unwrap();
        }
        let source = dir.join("bundle.json");
        fs::write(&source, manifest.to_string()).unwrap();
        let (_temp, closure) = qa::prepare(&f.context(), &source).unwrap().unwrap();
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(
            fs::File::open(closure).unwrap(),
        ));
        let paths: Vec<_> = archive
            .entries()
            .unwrap()
            .map(|entry| entry.unwrap().path().unwrap().into_owned())
            .collect();
        assert_eq!(paths.into_iter().collect::<BTreeSet<_>>(), expected);
        let paths = expected;
        assert!(paths.contains(&PathBuf::from("qa/bundle.json")));
        assert!(
            !paths
                .iter()
                .any(|path| path.to_string_lossy().contains("PASS"))
        );
    }

    #[test]
    fn a_path_outside_the_roots_is_refused_before_anything_is_hashed() {
        let f = fixture();
        let outside = f._dir.path().canonicalize().unwrap().join("stray.pdf");
        fs::write(&outside, b"bytes").unwrap();
        let error = publish(&f.context(), &outside).unwrap_err();
        assert!(matches!(error, PublishError::Path(_)), "got {error:?}");
        assert!(f.store.list_for_task("cas-b72a").unwrap().is_empty());
    }

    #[test]
    fn a_file_in_the_checkout_is_publishable() {
        let f = fixture();
        let file = f.project_root.join("docs").join("report.html");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        fs::write(&file, b"<html>").unwrap();
        let outcome = publish(&f.context(), &file).unwrap();
        assert_eq!(outcome.artifact.mime, "text/html");
    }

    #[test]
    fn two_publishes_of_the_same_bytes_get_distinct_records() {
        let f = fixture();
        let file = f.task_dir.join("brief.pdf");
        fs::write(&file, b"same bytes").unwrap();
        let first = publish(&f.context(), &file).unwrap();
        let second = publish(&f.context(), &file).unwrap();
        assert_ne!(
            first.artifact.id, second.artifact.id,
            "republishing must be citable separately"
        );
        assert_eq!(first.artifact.sha256, second.artifact.sha256);
        assert_eq!(f.store.list_for_task("cas-b72a").unwrap().len(), 2);
    }

    #[test]
    fn media_types_come_from_the_extension_and_unknown_is_honest() {
        assert_eq!(mime_for(Path::new("a/b.pdf")), "application/pdf");
        assert_eq!(mime_for(Path::new("a/b.PDF")), "application/pdf");
        assert_eq!(mime_for(Path::new("a/b.html")), "text/html");
        assert_eq!(mime_for(Path::new("a/b.md")), "text/markdown");
        assert_eq!(mime_for(Path::new("a/b.png")), "image/png");
        assert_eq!(
            mime_for(Path::new("a/b.wat")),
            "application/octet-stream",
            "an unknown extension must not be guessed into something renderable"
        );
        assert_eq!(mime_for(Path::new("a/noext")), "application/octet-stream");
    }

    #[test]
    fn typescript_sources_publish_as_text_while_transport_streams_remain_video() {
        let f = fixture();
        let mut transport_stream = vec![0xff; 188 * 2];
        for packet in transport_stream.chunks_exact_mut(188) {
            // Two valid null packets: sync, null PID, payload-only header.
            packet[..4].copy_from_slice(&[0x47, 0x1f, 0xff, 0x10]);
        }
        for (name, bytes, expected) in [
            ("probe.ts", b"export const probe = true;\n".as_slice(), "text/plain"),
            ("component.TSX", b"export const View = () => <div />;\n".as_slice(), "text/plain"),
            // ASCII G is also 0x47, but source is not a transport stream.
            ("graph.ts", b"Graph.render();\n".as_slice(), "text/plain"),
            ("recording.ts", transport_stream.as_slice(), "video/mp2t"),
        ] {
            let file = f.task_dir.join(name);
            fs::write(&file, bytes).unwrap();
            let outcome = publish(&f.context(), &file).unwrap();
            assert_eq!(outcome.artifact.mime, expected, "{name}");
            assert_eq!(f.store.get(&outcome.artifact.id).unwrap().unwrap().mime, expected, "{name}");
        }
    }

    #[tokio::test]
    async fn webm_publish_is_rejected_before_cloud_requests_and_suggests_zip() {
        let server = wiremock::MockServer::start().await;
        let endpoint = server.uri();
        let (outcome, stored_status) = tokio::task::spawn_blocking(move || {
            let f = fixture();
            let file = f.task_dir.join("receipt.webm");
            fs::write(&file, b"webm bytes").unwrap();
            let context = PublishContext {
                store: &f.store,
                roots: f.roots.clone(),
                task_id: "cas-b72a".to_string(),
                cloud: Some(ArtifactUploadClient::new(&endpoint, "test-token")),
            };

            let outcome = publish(&context, &file).unwrap();
            let stored_status = f
                .store
                .get(&outcome.artifact.id)
                .unwrap()
                .expect("publish retains the local artifact record")
                .status;
            (outcome, stored_status)
        })
        .await
        .unwrap();

        assert_eq!(outcome.artifact.mime, "video/webm");
        let reason = match &outcome.disposition {
            PublishDisposition::UploadFailed { reason } => reason,
            other => panic!("expected an upload failure, got {other:?}"),
        };
        assert!(reason.contains("unsupported_content_type"), "{reason}");
        assert!(reason.contains("application/zip"), "{reason}");
        assert_eq!(stored_status, "local");
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "an unsupported MIME must be refused before begin, presign, or PUT"
        );
    }

    #[test]
    fn every_disposition_reports_whether_cloud_commit_completed() {
        let cases = [
            PublishDisposition::Committed {
                cloud_url: Some("https://c/x".to_string()),
            },
            PublishDisposition::Committed { cloud_url: None },
            PublishDisposition::CommittedMetadataUnavailable {
                reason: "task reference unavailable".into(),
            },
            PublishDisposition::Committed { cloud_url: None }
                .with_qa_error(Some("too large".into())),
            PublishDisposition::NotLoggedIn.with_qa_error(Some("missing trace".into())),
            PublishDisposition::StorageNotLive {
                reason: "not live".to_string(),
            },
            PublishDisposition::NotLoggedIn,
            PublishDisposition::UploadFailed {
                reason: "boom".to_string(),
            },
        ];
        for case in cases {
            let summary = case.summary();
            assert!(!summary.is_empty());
            assert_eq!(
                case.is_committed(),
                matches!(
                    case.storage_disposition(),
                    StorageDisposition::Committed
                        | StorageDisposition::CommittedMetadataUnavailable { .. }
                )
            );
        }
    }
}

/// cassy#910: publish end to end against a Cloud double — begin, the PUT to
/// the object store, complete with the server's own sha256 check — then a
/// signed view URL for the committed record.
#[cfg(test)]
mod cloud_double_tests {
    use super::*;
    use std::fs;
    use std::sync::{Arc, Mutex};
    use tempfile::TempDir;
    use wiremock::matchers::{body_string_contains, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// sha256("hello world")
    const HELLO_SHA256: &str = "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9";

    thread_local! {
        static COMPLETION_WRITER: std::cell::RefCell<Option<Arc<Mutex<rusqlite::Connection>>>> = const { std::cell::RefCell::new(None) };
    }

    fn release_completion_writer(_: i32) -> bool {
        COMPLETION_WRITER.with(|writer| {
            writer
                .borrow_mut()
                .take()
                .unwrap()
                .lock()
                .unwrap()
                .execute_batch("COMMIT")
                .unwrap();
        });
        // The first UPDATE really fails with SQLITE_BUSY; only a retry succeeds.
        false
    }

    struct Fixture {
        _dir: TempDir,
        store: SqliteArtifactStore,
        roots: PublishRoots,
        file: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = TempDir::new().unwrap();
        let base = dir.path().canonicalize().unwrap();
        let cas_root = base.join("project").join(".cas");
        let artifacts_root = base.join("artifacts");
        let task_dir = crate::config::project_factory_artifacts_root(&cas_root, &artifacts_root)
            .join("cas-29624");
        fs::create_dir_all(&cas_root).unwrap();
        fs::create_dir_all(&task_dir).unwrap();
        let file = task_dir.join("report.pdf");
        fs::write(&file, b"hello world").unwrap();
        crate::store::open_task_store(&cas_root)
            .unwrap()
            .add(&crate::types::Task::new(
                "cas-29624".into(),
                "artifact".into(),
            ))
            .unwrap();
        Fixture {
            store: SqliteArtifactStore::open(&cas_root).unwrap(),
            roots: PublishRoots::new(&cas_root, &artifacts_root, "cas-29624"),
            file,
            _dir: dir,
        }
    }

    /// The double answers exactly as petra-stella-cloud#84 does: 201 from
    /// begin, and a complete body without a durable URL.
    async fn cloud_double(complete_status: u16, complete_body: serde_json::Value) -> MockServer {
        cloud_double_with_writer(complete_status, complete_body, None).await
    }

    async fn cloud_double_with_writer(
        complete_status: u16,
        complete_body: serde_json::Value,
        writer: Option<Arc<Mutex<rusqlite::Connection>>>,
    ) -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/artifacts/begin"))
            .and(header("Authorization", "Bearer test-tok"))
            .and(body_string_contains("\"task_id\":\"cas-29624\""))
            .and(body_string_contains(HELLO_SHA256))
            .and(body_string_contains("\"size_bytes\":11"))
            .and(body_string_contains("\"team_id\":\"team-7\""))
            .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                "artifact_id": "5f0c2b1e-cloud",
                "upload_url": format!("{}/blob/put?signature=upload-sig", server.uri()),
                "required_headers": { "x-content-type": "application/pdf" },
                "expires_at": "2026-09-24T21:00:00.000Z",
                "max_size_bytes": 26214400
            })))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PUT"))
            .and(path("/blob/put"))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let response = ResponseTemplate::new(complete_status).set_body_json(complete_body);
        Mock::given(method("POST"))
            .and(path("/api/artifacts/5f0c2b1e-cloud/complete"))
            .and(body_string_contains(HELLO_SHA256))
            .respond_with(move |_: &wiremock::Request| {
                if let Some(writer) = &writer {
                    writer
                        .lock()
                        .unwrap()
                        .execute_batch("BEGIN IMMEDIATE")
                        .unwrap();
                }
                response.clone()
            })
            .expect(1)
            .mount(&server)
            .await;
        server
    }

    fn client(server: &MockServer) -> ArtifactUploadClient {
        ArtifactUploadClient::new(&server.uri(), "test-tok").with_scope(
            Some("team-7".to_string()),
            Some("github.com/richards-llc/cassy".to_string()),
        )
    }

    #[tokio::test]
    async fn publish_commits_through_the_cloud_double_and_the_record_opens_a_signed_view_url() {
        let f = fixture();
        let database = f.roots.project_root.join(".cas/cas.db");
        let writer = Arc::new(Mutex::new(rusqlite::Connection::open(&database).unwrap()));
        let server = cloud_double_with_writer(
            200,
            serde_json::json!({
                "artifact_id": "5f0c2b1e-cloud",
                "status": "committed",
                "size_bytes": 11,
                "sha256": HELLO_SHA256
            }),
            Some(writer.clone()),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/api/artifacts/5f0c2b1e-cloud/url"))
            .and(header("Authorization", "Bearer test-tok"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "artifact_id": "5f0c2b1e-cloud",
                "url": format!("{}/blob/view?signature=view-sig", server.uri()),
                "expires_at": "2026-09-24T21:10:00.000Z",
                "name": "report.pdf",
                "mime": "application/pdf",
                "size_bytes": 11
            })))
            .expect(1)
            .mount(&server)
            .await;

        let client = client(&server);
        let (outcome, view) = tokio::task::spawn_blocking(move || {
            let pooled = cas_store::shared_db::shared_connection(&database).unwrap();
            COMPLETION_WRITER.with(|slot| *slot.borrow_mut() = Some(writer));
            pooled
                .lock()
                .unwrap()
                .busy_handler(Some(release_completion_writer))
                .unwrap();
            let context = PublishContext {
                store: &f.store,
                roots: f.roots.clone(),
                task_id: "cas-29624".to_string(),
                cloud: Some(client.clone()),
            };
            let outcome = publish(&context, &f.file).expect("publish");
            cas_store::shared_db::install_busy_handler(&pooled.lock().unwrap()).unwrap();
            assert!(outcome.disposition.is_committed(), "{outcome:?}");
            let task = crate::store::open_task_store(&f.roots.project_root.join(".cas"))
                .unwrap()
                .get("cas-29624")
                .unwrap();
            assert!(
                task.notes.contains(&outcome.artifact.id) && task.notes.contains("ARTIFACT_PROOF:"),
                "plain publish must carry its committed metadata to peers: {}",
                task.notes
            );
            assert_eq!(f.store.list_for_task("cas-29624").unwrap().len(), 1);
            let (note, warnings) = proof::prepare_note(
                &f.store,
                "cas-29624",
                &outcome.artifact.id,
                &f.roots.task_artifacts_dir,
                false,
            )
            .unwrap();
            assert!(warnings.is_empty(), "{warnings:?}");
            assert_eq!(note.matches("ARTIFACT_PROOF:").count(), 1, "{note}");
            let view = signed_view(&f.store, Some(&client), &outcome.artifact.id);
            (outcome, view.map(|view| view.view))
        })
        .await
        .unwrap();

        assert_eq!(
            outcome.disposition,
            PublishDisposition::Committed { cloud_url: None }
        );
        assert_eq!(outcome.artifact.status, "committed");
        assert_eq!(outcome.artifact.sha256, HELLO_SHA256);
        assert_eq!(
            outcome.artifact.cloud_artifact_id.as_deref(),
            Some("5f0c2b1e-cloud"),
            "the record keeps Cloud's artifact id"
        );
        let view = view.expect("a committed record has a signed view URL");
        assert!(view.url.ends_with("/blob/view?signature=view-sig"));
        assert_eq!(view.name.as_deref(), Some("report.pdf"));
    }

    #[tokio::test]
    async fn completion_retry_exhaustion_keeps_the_uploaded_record_honest_cas_9968() {
        let f = fixture();
        let database = f.roots.project_root.join(".cas/cas.db");
        let writer = Arc::new(Mutex::new(rusqlite::Connection::open(&database).unwrap()));
        let server = cloud_double_with_writer(
            200,
            serde_json::json!({"status": "committed"}),
            Some(writer.clone()),
        )
        .await;
        let client = client(&server);
        tokio::task::spawn_blocking(move || {
            let pooled = cas_store::shared_db::shared_connection(&database).unwrap();
            pooled
                .lock()
                .unwrap()
                .busy_timeout(std::time::Duration::ZERO)
                .unwrap();
            let outcome = publish(
                &PublishContext {
                    store: &f.store,
                    roots: f.roots.clone(),
                    task_id: "cas-29624".into(),
                    cloud: Some(client),
                },
                &f.file,
            )
            .unwrap();
            cas_store::shared_db::install_busy_handler(&pooled.lock().unwrap()).unwrap();
            writer.lock().unwrap().execute_batch("COMMIT").unwrap();
            assert_eq!(outcome.artifact.status, "uploaded");
            assert!(!outcome.disposition.is_committed());
            let PublishDisposition::UploadFailed { reason } = &outcome.disposition else {
                panic!("{outcome:?}")
            };
            assert!(
                reason.contains("uploaded, not committed locally"),
                "{reason}"
            );
            assert_eq!(f.store.list_for_task("cas-29624").unwrap().len(), 1);
            let (note, warnings) = proof::prepare_note(
                &f.store,
                "cas-29624",
                &outcome.artifact.id,
                &f.roots.task_artifacts_dir,
                false,
            )
            .unwrap();
            assert!(!note.contains("ARTIFACT_PROOF:"), "{note}");
            assert_eq!(warnings.len(), 1);
        })
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn xz_publish_is_rejected_before_presign_when_cloud_does_not_allow_detected_mime() {
        let server = MockServer::start().await;
        let client = client(&server);
        let (outcome, stored_status) = tokio::task::spawn_blocking(move || {
            let f = fixture();
            let file = f.file.with_file_name("live-evidence.tar.xz");
            fs::write(&file, b"xz evidence bytes").unwrap();
            let context = PublishContext {
                store: &f.store,
                roots: f.roots.clone(),
                task_id: "cas-29624".to_string(),
                cloud: Some(client.clone()),
            };

            let outcome = publish(&context, &file).expect("publish keeps a local record");
            let stored_status = f
                .store
                .get(&outcome.artifact.id)
                .unwrap()
                .expect("publish retains the local artifact record")
                .status;
            (outcome, stored_status)
        })
        .await
        .unwrap();

        assert_eq!(outcome.artifact.mime, "application/x-xz");
        let reason = match &outcome.disposition {
            PublishDisposition::UploadFailed { reason } => reason,
            other => panic!("expected a named unsupported-MIME refusal, got {other:?}"),
        };
        assert!(reason.contains("unsupported_content_type"), "{reason}");
        assert!(reason.contains("application/x-xz"), "{reason}");
        assert_eq!(stored_status, "local");
        assert!(
            server.received_requests().await.unwrap().is_empty(),
            "an unsupported detected MIME must be refused before presign or PUT"
        );
    }

    #[tokio::test]
    async fn a_digest_the_server_rejects_leaves_the_record_out_of_cloud_and_unviewable() {
        let server = cloud_double(
            409,
            serde_json::json!({
                "error": "digest_mismatch",
                "status": "rejected",
                "declared": { "sha256": HELLO_SHA256, "size_bytes": 11 },
                "stored": { "sha256": "0".repeat(64), "size_bytes": 11 }
            }),
        )
        .await;

        let client = client(&server);
        let (outcome, view) = tokio::task::spawn_blocking(move || {
            let f = fixture();
            let context = PublishContext {
                store: &f.store,
                roots: f.roots.clone(),
                task_id: "cas-29624".to_string(),
                cloud: Some(client.clone()),
            };
            let outcome = publish(&context, &f.file).expect("publish still records");
            let view = signed_view(&f.store, Some(&client), &outcome.artifact.id);
            (outcome, view)
        })
        .await
        .unwrap();

        match &outcome.disposition {
            PublishDisposition::UploadFailed { reason } => {
                assert!(reason.contains(&"0".repeat(64)), "{reason}")
            }
            other => panic!("expected UploadFailed, got {other:?}"),
        }
        assert_eq!(outcome.artifact.status, "uploaded");
        match view.expect_err("never committed") {
            ViewError::NotInCloud { status } => assert_eq!(status, "uploaded"),
            other => panic!("expected NotInCloud, got {other:?}"),
        }
    }

    #[test]
    fn a_local_record_or_an_unknown_id_is_answered_without_the_network() {
        let f = fixture();
        let context = PublishContext {
            store: &f.store,
            roots: f.roots.clone(),
            task_id: "cas-29624".to_string(),
            cloud: None,
        };
        let outcome = publish(&context, &f.file).unwrap();
        // An endpoint nothing listens on: any network call would fail loudly
        // as a Cloud error rather than the answers asserted here.
        let offline = ArtifactUploadClient::new("http://127.0.0.1:9", "t");
        match signed_view(&f.store, Some(&offline), &outcome.artifact.id) {
            Err(ViewError::NotInCloud { status }) => assert_eq!(status, "local"),
            other => panic!("expected NotInCloud, got {other:?}"),
        }
        assert!(matches!(
            signed_view(&f.store, Some(&offline), "art-missing"),
            Err(ViewError::Unknown(_))
        ));
    }
}
