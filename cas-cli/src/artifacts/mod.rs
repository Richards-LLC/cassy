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

pub mod cloud;
pub mod paths;

use cas_store::{NewArtifact, PublishedArtifact, SqliteArtifactStore};
use cas_types::{ARTIFACT_MAX_BYTES, ArtifactRef};
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use cloud::{ArtifactDigest, ArtifactUploadClient, BeginRequest, UploadFailure};
use paths::{PublishRoots, resolve_publishable_path};

/// 64 KiB: large enough that hashing a 25 MiB file is a handful of syscalls,
/// small enough not to matter on a constrained host.
const HASH_CHUNK: usize = 64 * 1024;

/// What a publish did, in the operator's terms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublishDisposition {
    /// Recorded locally and committed to Cloud storage.
    Committed { cloud_url: Option<String> },
    /// Recorded locally; Cloud storage is not live on this endpoint yet.
    StorageNotLive { reason: String },
    /// Recorded locally; this installation has no cloud credentials.
    NotLoggedIn,
    /// Recorded locally; the upload was attempted and did not complete. The
    /// record is still usable, and the reason says what to do.
    UploadFailed { reason: String },
}

impl PublishDisposition {
    /// Every disposition is a successful publish: the record exists and can be
    /// referenced. Only the *storage* outcome differs.
    pub fn is_committed(&self) -> bool {
        matches!(self, PublishDisposition::Committed { .. })
    }

    /// One line for a person, naming what happened and what follows from it.
    pub fn summary(&self) -> String {
        match self {
            PublishDisposition::Committed {
                cloud_url: Some(url),
            } => {
                format!("stored in Cloud at {url}")
            }
            PublishDisposition::Committed { cloud_url: None } => {
                "stored in Cloud (private)".to_string()
            }
            PublishDisposition::StorageNotLive { .. } => {
                "recorded locally; Cloud storage is not live yet".to_string()
            }
            PublishDisposition::NotLoggedIn => {
                "recorded locally; not logged in to Cassy Cloud".to_string()
            }
            PublishDisposition::UploadFailed { reason } => {
                format!("recorded locally; the upload did not complete: {reason}")
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

/// Media type from the file extension. Unknown extensions get the honest
/// `application/octet-stream` rather than a guess.
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
        "json" => "application/json",
        "jsonl" | "ndjson" => "application/x-ndjson",
        "csv" => "text/csv",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
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
            disposition: PublishDisposition::NotLoggedIn,
            source,
        });
    };

    let disposition = match upload(client, context.store, &artifact, &source) {
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

    Ok(PublishOutcome {
        artifact,
        disposition,
        source,
    })
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

    if let Err(error) = store.mark_committed(&artifact.id, completed.url.as_deref()) {
        return Err(UploadFailure::Failed {
            step: cloud::UploadStep::Complete,
            reason: format!("could not record the completion locally: {error}"),
            interaction: format!("artifact {}", artifact.id),
        });
    }
    Ok(completed.url)
}

#[cfg(test)]
mod tests {
    use super::*;
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
        let task_dir = artifacts_root.join("cas-b72a");
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
    fn every_disposition_reads_as_a_successful_publish_with_a_storage_caveat() {
        let cases = [
            PublishDisposition::Committed {
                cloud_url: Some("https://c/x".to_string()),
            },
            PublishDisposition::Committed { cloud_url: None },
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
                matches!(case, PublishDisposition::Committed { .. })
            );
        }
    }
}
