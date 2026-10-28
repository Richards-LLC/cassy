//! Portable proof references carried by the task's existing synced notes.
//! The bytes still use the published-artifact ledger and Cloud transport.
use super::{SignedView, cloud::ArtifactUploadClient, digest_file, paths::PublishRoots};
use cas_store::{NewArtifact, PublishedArtifact, SqliteArtifactStore};
use cas_types::ArtifactRef;
use std::path::{Path, PathBuf};

const MARKER: &str = "ARTIFACT_PROOF: ";

/// A missing local pointer says nothing about whether Cloud has the bytes.
#[derive(Debug, thiserror::Error)]
#[error("artifact_metadata_unavailable: {artifact_id}\nThis machine lacks synced metadata; Cloud bytes have not been checked.\nRun `cas cloud sync`, then retry.")]
pub struct MetadataUnavailable {
    pub artifact_id: String,
    pub task_id: Option<String>,
}

/// Share every successful publication through the existing trusted task-note
/// transport, including publications that did not request a prose proof note.
pub(crate) fn record_published(cas_root: &Path, artifact: &PublishedArtifact) -> anyhow::Result<()> {
    validate(artifact, &artifact.task_id)?;
    let tasks = crate::store::open_task_store(cas_root)?;
    let note = format!(
        "[{}] Published artifact {}\nARTIFACT_PROOF: {}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M"),
        artifact.id,
        serde_json::to_string(artifact)?
    );
    tasks.append_trusted_note(&artifact.task_id, &note)?;
    Ok(())
}

/// Resolve metadata before attempting any byte transfer. Show and fetch share
/// this reader so a synced reference is inspectable before its first download.
pub fn lookup_metadata(
    cas_root: &Path,
    task_id: Option<&str>,
    id: &str,
) -> anyhow::Result<PublishedArtifact> {
    let store = SqliteArtifactStore::open(cas_root)?;
    let local = store.get(id)?;
    if task_id.is_none() {
        if let Some(artifact) = local {
            return Ok(artifact);
        }
    }
    let tasks = crate::store::open_task_store_local(cas_root)?;
    let mut found = None;
    if let Some(task_id) = task_id {
        let task = tasks.get(task_id)?;
        found = from_notes(&task.notes, id, task_id)?;
        if let (Some(reference), Some(local)) = (&found, &local) {
            anyhow::ensure!(
                same_artifact(reference, local),
                "artifact ledger conflicts with proof reference"
            );
        }
        found = found.or(local);
        if let Some(artifact) = &found {
            anyhow::ensure!(
                artifact.task_id == task_id,
                "artifact belongs to another task"
            );
        }
    } else {
        // Match task-reader visibility, including accepted project proposals.
        let scope = crate::mcp::tools::core::task::ProjectScope::current(cas_root);
        for task in tasks.list(None)?.into_iter().filter(|task| scope.shows(task)) {
            if let Some(reference) = from_notes(&task.notes, id, &task.id)? {
                if let Some(previous) = &found {
                    anyhow::ensure!(
                        same_artifact(previous, &reference),
                        "conflicting proof artifact references for {id}"
                    );
                }
                found = Some(reference);
            }
        }
    }
    found.ok_or_else(|| {
        MetadataUnavailable {
            artifact_id: id.to_string(),
            task_id: task_id.map(str::to_string),
        }
        .into()
    })
}

pub fn contains_proof_marker(note: &str) -> bool {
    note.lines().any(|line| line.starts_with("ARTIFACT_PROOF:"))
}

pub(crate) fn same_artifact(a: &PublishedArtifact, b: &PublishedArtifact) -> bool {
    a.id == b.id
        && a.task_id == b.task_id
        && a.name == b.name
        && a.mime == b.mime
        && a.size_bytes == b.size_bytes
        && a.sha256 == b.sha256
        && a.cloud_artifact_id == b.cloud_artifact_id
}

pub(crate) fn validate(artifact: &PublishedArtifact, task_id: &str) -> anyhow::Result<()> {
    ArtifactRef {
        artifact_id: artifact.id.clone(),
        name: artifact.name.clone(),
        mime: artifact.mime.clone(),
        size_bytes: artifact.size_bytes,
        sha256: artifact.sha256.clone(),
    }
    .validate()?;
    anyhow::ensure!(
        artifact.id.starts_with("art-")
            && artifact
                .id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-'),
        "invalid artifact id"
    );
    anyhow::ensure!(
        !task_id.is_empty()
            && task_id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "invalid task id"
    );
    anyhow::ensure!(
        artifact.task_id == task_id,
        "artifact belongs to another task"
    );
    anyhow::ensure!(
        artifact.status == "committed"
            && artifact
                .cloud_artifact_id
                .as_deref()
                .is_some_and(|id| !id.is_empty()),
        "artifact is not committed to Cloud; publish it while logged in before sharing proof"
    );
    Ok(())
}

/// A pulled owner row can carry proof lines that were written by the source
/// machine's trusted publisher. Carry each validated record with its complete
/// note so the trusted sink can upgrade the raw copy without detaching proof.
pub(crate) fn take_published_proof_notes(notes: &mut String, task_id: &str) -> Vec<String> {
    let mut trusted = Vec::new();
    let mut ordinary = Vec::new();
    for block in cas_store::task_note_markers::note_blocks(notes) {
        let proofs = block.lines().filter(|line| {
            line.strip_prefix(MARKER)
                .and_then(|json| serde_json::from_str::<PublishedArtifact>(json).ok())
                .is_some_and(|record| validate(&record, task_id).is_ok())
        }).collect::<Vec<_>>();
        if !proofs.is_empty() {
            // Trust only the records validated above, never other marker lines
            // that happen to share the same incoming note.
            trusted.push(cas_store::task_note_markers::neutralize_new_markers(
                block, Some(&proofs.join("\n")),
            ).into_owned());
        }
        ordinary.push(block.lines().filter(|line| !proofs.contains(line)).collect::<Vec<_>>().join("\n"));
    }
    if !trusted.is_empty() {
        *notes = ordinary.into_iter().filter(|block| !block.is_empty()).collect::<Vec<_>>().join("\n\n");
    }
    trusted
}

/// Add the committed ledger record beside a cited id. This also upgrades a
/// local path citation if its bytes already have a committed artifact record.
/// Missing peer paths still warn: existence on this host is not required.
pub fn prepare_note(
    store: &SqliteArtifactStore,
    task_id: &str,
    note: &str,
    artifacts_base: &Path,
    warn_local_paths: bool,
) -> anyhow::Result<(String, Vec<String>)> {
    let records = store.list_for_task(task_id)?;
    let mut attached = std::collections::BTreeMap::new();
    let mut warnings = Vec::new();
    for id in note.split(|c: char| !c.is_ascii_alphanumeric() && c != '-') {
        if let Some(record) = records.iter().find(|a| a.id == id) {
            if validate(record, task_id).is_ok() {
                attached.insert(record.id.clone(), record.clone());
            } else {
                warnings.push(format!(
                    "{id} is machine-local; publish it to Cloud before sharing proof"
                ));
            }
        }
    }
    for (index, segment) in note.split('`').enumerate() {
        let tokens = if index % 2 == 1 {
            vec![segment]
        } else {
            segment.split_whitespace().collect()
        };
        for token in tokens {
            let token = token.trim_matches(|c: char| {
                matches!(c, '(' | ')' | '[' | ']' | ',' | ';' | '"' | '\'' | '.')
            });
            let token = token.split_once('=').map_or(token, |(_, value)| value);
            let expanded = crate::qa_evidence::expand_home(token);
            let path = expanded.as_path();
            let machine_local = path.starts_with(artifacts_base)
                || token.starts_with("~/.cas/artifacts/")
                || token.contains("/.cas/artifacts/");
            if !machine_local {
                continue;
            }
            let published = std::fs::metadata(path)
                .ok()
                .filter(|m| m.is_file() && m.len() <= cas_types::ARTIFACT_MAX_BYTES)
                .and_then(|_| digest_file(path).ok())
                .and_then(|(sha, size)| {
                    records.iter().find(|a| {
                        a.sha256 == sha && a.size_bytes == size && validate(a, task_id).is_ok()
                    })
                });
            if let Some(record) = published {
                attached.insert(record.id.clone(), record.clone());
            } else if warn_local_paths {
                warnings.push(format!("{token} is machine-local and unpublished; use artifact action=publish task_id={task_id} path=<log> note_type=platform_proof notes=<receipt>"));
            }
        }
    }
    let mut body = note.to_string();
    for record in attached.values() {
        body.push('\n');
        body.push_str(MARKER);
        body.push_str(&serde_json::to_string(record)?);
    }
    Ok((body, warnings))
}

/// A receiving host needs only the synced note, never the publisher's path
/// or SQLite file. Reject conflicting records rather than choosing one.
pub(crate) fn from_notes(notes: &str, id: &str, task_id: &str) -> anyhow::Result<Option<PublishedArtifact>> {
    let mut found = None;
    for line in notes.lines().filter_map(|line| line.strip_prefix(MARKER)) {
        let record: PublishedArtifact = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(error) => {
                tracing::warn!(task_id, error = %error, "skipping malformed artifact proof note");
                continue;
            }
        };
        if record.id != id {
            continue;
        }
        validate(&record, task_id)?;
        if let Some(previous) = &found {
            anyhow::ensure!(
                same_artifact(previous, &record),
                "conflicting proof artifact references for {id}"
            );
        }
        found = Some(record);
    }
    Ok(found)
}

/// Download through the existing verified artifact path, save atomically,
/// and import into the existing ledger so show/list and close citations work.
fn fetch(
    store: &SqliteArtifactStore,
    cloud: Option<&ArtifactUploadClient>,
    roots: &PublishRoots,
    artifact: PublishedArtifact,
) -> anyhow::Result<PathBuf> {
    validate(&artifact, &artifact.task_id)?;
    let existing = store.get(&artifact.id)?;
    if let Some(existing) = &existing {
        anyhow::ensure!(
            same_artifact(existing, &artifact),
            "artifact ledger conflicts with proof reference"
        );
    }
    let cloud_id = artifact
        .cloud_artifact_id
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("artifact is not committed to Cloud"))?;
    let client = cloud.ok_or_else(|| anyhow::anyhow!("not logged in to Cassy Cloud"))?;
    let view = client.view_url(cloud_id)?;
    anyhow::ensure!(
        view.artifact_id == cloud_id,
        "Cloud returned another artifact id"
    );
    let signed = SignedView {
        artifact: artifact.clone(),
        view,
    };
    let bytes = super::slack::download(&signed)?;

    let task_dir = &roots.task_artifacts_dir;
    std::fs::create_dir_all(task_dir)?;
    let base = roots
        .legacy_task_artifacts_dir
        .parent()
        .ok_or_else(|| anyhow::anyhow!("missing artifacts root"))?;
    anyhow::ensure!(
        crate::config::canonical_factory_task_artifact_dir(base, task_dir).is_some(),
        "task artifact directory escapes its configured namespace"
    );
    let destination = task_dir.join(format!("{}-{}", artifact.id, artifact.name));
    let mut staging = tempfile::NamedTempFile::new_in(task_dir)?;
    std::io::Write::write_all(&mut staging, &bytes)?;
    staging.persist(&destination)?;
    match existing {
        None => {
            store.record_local(&NewArtifact {
                id: artifact.id.clone(),
                task_id: artifact.task_id.clone(),
                name: artifact.name.clone(),
                mime: artifact.mime.clone(),
                size_bytes: artifact.size_bytes,
                sha256: artifact.sha256.clone(),
            })?;
            store.mark_uploaded(&artifact.id, cloud_id)?;
            store.mark_committed(&artifact.id, None)?;
        }
        Some(_) => {}
    }
    Ok(destination)
}

/// Shared CLI/MCP entry: prefer the synced task reference when a task is named.
pub fn fetch_task(
    cas_root: &Path,
    task_id: &str,
    id: &str,
) -> anyhow::Result<(PublishedArtifact, PathBuf)> {
    let store = SqliteArtifactStore::open(cas_root)?;
    let artifact = lookup_metadata(cas_root, Some(task_id), id)?;
    validate(&artifact, task_id)?;
    let config = crate::config::Config::load(cas_root).unwrap_or_default();
    let base =
        crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let roots = PublishRoots::new(cas_root, &base, task_id);
    let path = fetch(
        &store,
        super::cloud_client(cas_root).as_ref(),
        &roots,
        artifact.clone(),
    )?;
    Ok((artifact, path))
}
