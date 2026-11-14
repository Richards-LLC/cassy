//! Portable proof references carried by the task's existing synced notes.
//! The bytes still use the published-artifact ledger and Cloud transport.
use super::{SignedView, cloud::ArtifactUploadClient, digest_file, paths::PublishRoots};
use cas_store::{NewArtifact, PublishedArtifact, SqliteArtifactStore};
use cas_types::ArtifactRef;
use std::path::{Path, PathBuf};

const MARKER: &str = "ARTIFACT_PROOF: ";

/// A missing local pointer says nothing about whether Cloud has the bytes.
#[derive(Debug, thiserror::Error)]
#[error(
    "artifact_metadata_unavailable: {artifact_id}\n{reason}\nCloud bytes have not been checked."
)]
pub struct MetadataUnavailable {
    pub artifact_id: String,
    pub task_id: Option<String>,
    pub reason: &'static str,
}

/// Share every successful publication through the existing trusted task-note
/// transport, including publications that did not request a prose proof note.
pub(crate) fn record_published(
    cas_root: &Path,
    artifact: &PublishedArtifact,
) -> anyhow::Result<()> {
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
    if let Some(artifact) = local.as_ref().filter(|artifact| artifact.revocation.is_some()) {
        anyhow::ensure!(task_id.is_none_or(|task| task == artifact.task_id), "artifact belongs to another task");
        return Ok(artifact.clone());
    }
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
        for task in tasks
            .list(None)?
            .into_iter()
            .filter(|task| scope.shows(task))
        {
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
            reason: "This machine lacks synced metadata; run `cas cloud sync`, then retry.",
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
    super::require_available(artifact)?;
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
        let proofs = block
            .lines()
            .filter(|line| {
                line.strip_prefix(MARKER)
                    .and_then(|json| serde_json::from_str::<PublishedArtifact>(json).ok())
                    .is_some_and(|record| validate(&record, task_id).is_ok())
                    || super::qa::valid_proof(line, task_id)
            })
            .collect::<Vec<_>>();
        if !proofs.is_empty() {
            // Trust only the records validated above, never other marker lines
            // that happen to share the same incoming note.
            trusted.push(
                cas_store::task_note_markers::neutralize_new_markers(
                    block,
                    Some(&proofs.join("\n")),
                )
                .into_owned(),
            );
        }
        ordinary.push(
            block
                .lines()
                .filter(|line| !proofs.contains(line))
                .collect::<Vec<_>>()
                .join("\n"),
        );
    }
    if !trusted.is_empty() {
        *notes = ordinary
            .into_iter()
            .filter(|block| !block.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
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
pub(crate) fn from_notes(
    notes: &str,
    id: &str,
    task_id: &str,
) -> anyhow::Result<Option<PublishedArtifact>> {
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
    file_name: &str,
) -> anyhow::Result<PathBuf> {
    validate(&artifact, &artifact.task_id)?;
    let existing = store.get(&artifact.id)?;
    if let Some(existing) = &existing {
        super::require_available(existing)?;
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
    let view = client.view_url(cloud_id).map_err(|error| match error {
        super::ViewFailure::Revoked => anyhow::Error::from(super::ArtifactRevoked { artifact_id: artifact.id.clone() }),
        error => error.into(),
    })?;
    anyhow::ensure!(
        view.artifact_id == cloud_id,
        "Cloud returned another artifact id"
    );
    let signed = SignedView {
        artifact: artifact.clone(),
        view,
    };
    let bytes = super::slack::download(&signed)?;
    if let Some(current) = store.get(&artifact.id)? {
        super::require_available(&current)?;
    }

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
    let destination = task_dir.join(file_name);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
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
    let cloud = super::cloud_client(cas_root);
    let artifact = match lookup_metadata(cas_root, Some(task_id), id) {
        Ok(artifact) => artifact,
        Err(error)
            if error.downcast_ref::<MetadataUnavailable>().is_some()
                || matches!(
                    error.downcast_ref::<cas_store::StoreError>(),
                    Some(cas_store::StoreError::TaskNotFound(_))
                ) =>
        {
            let unavailable = || MetadataUnavailable {
                artifact_id: id.to_string(),
                task_id: Some(task_id.to_string()),
                reason: "The local task or proof is missing and the authenticated Cloud scope lookup is unavailable or incomplete.",
            };
            let client = cloud.as_ref().ok_or_else(unavailable)?;
            let deadline = std::time::Instant::now() + super::cloud::PROOF_LOOKUP_DEADLINE;
            let scoped_tasks = match client.configured_task_notes(task_id, deadline)
                .map_err(|_| unavailable())?
            {
                Some(scoped) if from_notes(&scoped.notes, id, task_id)?.is_some() => vec![scoped],
                _ => client.visible_task_notes(task_id, deadline).map_err(|_| unavailable())?,
            };
            let mut selected = None;
            for scoped in scoped_tasks {
                if let Some(proof) = from_notes(&scoped.notes, id, task_id)? {
                    anyhow::ensure!(
                        selected.is_none(),
                        "conflicting proof artifact references for {id} across visible Cloud scopes"
                    );
                    // Retain the authenticated owner scope with the proof until
                    // every scope has been checked for conflicting matches.
                    selected = Some((scoped.team_id, scoped.project_id, proof));
                }
            }
            selected
                .map(|(_, _, proof)| proof)
                .ok_or_else(unavailable)?
        }
        Err(error) => return Err(error),
    };
    validate(&artifact, task_id)?;
    let config = crate::config::Config::load(cas_root).unwrap_or_default();
    let base =
        crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let roots = PublishRoots::new(cas_root, &base, task_id);
    let file_name = format!("{}-{}", artifact.id, artifact.name);
    let path = fetch(&store, cloud.as_ref(), &roots, artifact.clone(), &file_name)?;
    Ok((artifact, path))
}

const EVIDENCE_SYNC: &str = "EVIDENCE_SYNC: ";

/// What a routed build's park published: its evidence directory and the
/// exact bytes the reviewing machine must hold before review (cas-e571).
#[derive(serde::Serialize, serde::Deserialize)]
struct EvidenceSync {
    machine_id: String,
    root: String,
    files: Vec<SyncedFile>,
}

/// One evidence file by its `/`-separated path under the task evidence
/// directory. Empty files carry no artifact: their bytes are the hash.
#[derive(serde::Serialize, serde::Deserialize)]
struct SyncedFile {
    name: String,
    sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    artifact_id: Option<String>,
}

/// A manifest path is task-relative and stays inside the directory.
fn safe_relative(name: &str) -> bool {
    !name.is_empty()
        && Path::new(name).components().all(|part| matches!(part, std::path::Component::Normal(_)))
}

/// Every regular file under `dir`, recursively, by task-relative path.
fn evidence_files(dir: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut files = Vec::new();
    for entry in walkdir::WalkDir::new(dir).min_depth(1).sort_by_file_name() {
        let entry = entry.map_err(std::io::Error::other)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(dir).map_err(std::io::Error::other)?;
        let name = relative.to_str().filter(|name| safe_relative(name)).ok_or_else(|| {
            std::io::Error::other(format!("{} has no portable task-relative name", entry.path().display()))
        })?;
        files.push((name.replace(std::path::MAIN_SEPARATOR, "/"), entry.path().to_path_buf()));
    }
    Ok(files)
}

fn evidence_dir(cas_root: &Path, task: &cas_types::Task) -> anyhow::Result<PathBuf> {
    crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::task_evidence_dir(cas_root, task)
        .map_err(anyhow::Error::msg)
}

/// cas-e571: a build routed from another machine publishes every file in its
/// task evidence directory through the artifact path at park, then records a
/// sha256 manifest, so the routing machine's reviewer can materialize it.
pub(crate) fn publish_routed_evidence(cas_root: &Path, task: &cas_types::Task) -> Result<(), String> {
    let Some(source) = crate::factory_build_routing::routed_from(task) else { return Ok(()) };
    let machine_id = cas_types::Agent::get_or_generate_machine_id();
    if source == machine_id {
        return Ok(());
    }
    let dir = evidence_dir(cas_root, task).map_err(|error| error.to_string())?;
    let fail = |detail: String| format!(
        "EVIDENCE SYNC FAILED: {detail}. This build was routed from {source}, whose reviewer reads \
         the task evidence directory under {source}'s artifacts root; this machine's is {}. \
         Re-run close once Cloud accepts the upload.", dir.display());
    if !dir.exists() {
        return Ok(());
    }
    let paths = evidence_files(&dir)
        .map_err(|error| fail(format!("cannot read {}: {error}", dir.display())))?;
    let store = SqliteArtifactStore::open(cas_root).map_err(|error| fail(error.to_string()))?;
    let config = crate::config::Config::load(cas_root).unwrap_or_default();
    let base = crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let context = super::PublishContext {
        store: &store,
        roots: PublishRoots::new(cas_root, &base, &task.id),
        task_id: task.id.clone(),
        cloud: super::cloud_client(cas_root),
    };
    let published = store.list_for_task(&task.id).map_err(|error| fail(error.to_string()))?;
    let mut files = Vec::new();
    for (name, path) in paths {
        let (sha256, size) = digest_file(&path).map_err(|error| fail(error.to_string()))?;
        if size == 0 {
            files.push(SyncedFile { name, sha256, artifact_id: None });
            continue;
        }
        let leaf = path.file_name().and_then(|leaf| leaf.to_str()).unwrap_or_default();
        let reused = published.iter().find(|record| {
            record.name == leaf && record.sha256 == sha256 && validate(record, &task.id).is_ok()
        });
        let artifact_id = match reused {
            Some(record) => record.id.clone(),
            None => {
                let outcome = super::publish(&context, &path)
                    .map_err(|error| fail(format!("{name}: {error}")))?;
                if outcome.disposition.storage_disposition() != super::StorageDisposition::Committed {
                    return Err(fail(format!("{name}: {}", outcome.disposition.summary())));
                }
                outcome.artifact.id
            }
        };
        files.push(SyncedFile { name, sha256, artifact_id: Some(artifact_id) });
    }
    let manifest = EvidenceSync { machine_id, root: dir.display().to_string(), files };
    let note = format!(
        "[{}] Evidence synced for review on {source}\n{EVIDENCE_SYNC}{}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M"),
        serde_json::to_string(&manifest).map_err(|error| fail(error.to_string()))?,
    );
    let tasks = crate::store::open_task_store(cas_root).map_err(|error| fail(error.to_string()))?;
    tasks.append_note(&task.id, &note).map_err(|error| fail(error.to_string()))?;
    Ok(())
}

/// The newest routed-park manifest restored byte-identical where the verifier
/// reads, or a refusal naming both machines' evidence directories (cas-e571).
fn materialize_manifest(
    cas_root: &Path,
    task: &cas_types::Task,
    sync: &EvidenceSync,
) -> Result<(), String> {
    let dir = evidence_dir(cas_root, task).map_err(|error| error.to_string())?;
    let mut missing = Vec::new();
    let store = SqliteArtifactStore::open(cas_root).map_err(|error| error.to_string())?;
    let config = crate::config::Config::load(cas_root).unwrap_or_default();
    let base = crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let roots = PublishRoots::new(cas_root, &base, &task.id);
    let cloud = super::cloud_client(cas_root);
    let identical = |file: &SyncedFile| {
        digest_file(&dir.join(&file.name)).is_ok_and(|(sha256, _)| sha256 == file.sha256)
    };
    for file in &sync.files {
        if !safe_relative(&file.name) {
            missing.push(format!("{} (unsafe path)", file.name));
            continue;
        }
        if identical(file) {
            continue;
        }
        let fetched = (|| -> anyhow::Result<()> {
            let Some(id) = &file.artifact_id else {
                let path = dir.join(&file.name);
                std::fs::create_dir_all(path.parent().unwrap_or(&dir))?;
                std::fs::write(path, b"")?;
                return Ok(());
            };
            let artifact = from_notes(&task.notes, id, &task.id)?
                .ok_or_else(|| anyhow::anyhow!("no synced artifact record {id}"))?;
            anyhow::ensure!(artifact.sha256 == file.sha256, "artifact {id} has other bytes");
            fetch(&store, cloud.as_ref(), &roots, artifact, &file.name)?;
            Ok(())
        })();
        match fetched {
            Ok(()) if identical(file) => {}
            Ok(()) => missing.push(format!("{} (fetched outside {})", file.name, dir.display())),
            Err(error) => missing.push(format!("{} ({error})", file.name)),
        }
    }
    if missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "EVIDENCE NOT SYNCED: {} was built on {} with evidence in {}, but this machine's evidence \
         directory {} lacks byte-identical {}. The pre-merge verifier was not dispatched; restore \
         the files there or have the worker re-run close, then retry the merge.",
        task.id, sync.machine_id, sync.root, dir.display(), missing.join(", "),
    ))
}

/// Before a pre-merge review, save each committed artifact another machine
/// published for this task (absent from this ledger) under its published name
/// in this machine's task evidence directory, which the verifier reads
/// (cas-5601). Local files win. A failure leaves the verifier's own
/// missing-evidence path; it never blocks the dispatch. A routed park's
/// manifest is the exception: it must arrive byte-identical (cas-e571).
pub(crate) fn materialize_peer_evidence(cas_root: &Path, task: &cas_types::Task) -> Result<(), String> {
    let manifest = task.notes.lines().rev().find_map(|line| {
        serde_json::from_str::<EvidenceSync>(line.strip_prefix(EVIDENCE_SYNC)?).ok()
    });
    if let Some(sync) = manifest {
        return materialize_manifest(cas_root, task, &sync);
    }
    let result = (|| -> anyhow::Result<()> {
        let store = SqliteArtifactStore::open(cas_root)?;
        let mut ids: Vec<String> = task.notes.lines()
            .filter_map(|line| line.strip_prefix(MARKER))
            .filter_map(|line| serde_json::from_str::<PublishedArtifact>(line).ok())
            .map(|record| record.id)
            .collect();
        ids.sort();
        ids.dedup();
        let config = crate::config::Config::load(cas_root).unwrap_or_default();
        let base = crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
        let roots = PublishRoots::new(cas_root, &base, &task.id);
        let cloud = super::cloud_client(cas_root);
        for id in ids {
            let Some(artifact) = from_notes(&task.notes, &id, &task.id)? else { continue };
            let name = artifact.name.clone();
            if store.get(&id)?.is_some() || Path::new(&name).file_name() != Some(name.as_ref())
                || roots.task_artifacts_dir.join(&name).exists()
            {
                continue;
            }
            if let Err(error) = fetch(&store, cloud.as_ref(), &roots, artifact, &name) {
                tracing::warn!(task_id = %task.id, artifact_id = %id, %error, "peer evidence not materialized");
            }
        }
        Ok(())
    })();
    if let Err(error) = result {
        tracing::warn!(task_id = %task.id, %error, "peer evidence not materialized");
    }
    Ok(())
}

/// A supervisor machine reviewing a peer's parked delivery: its own repo with
/// a one-commit delivery, its own `.cas`, and Cloud at `endpoint`. Tests set
/// `CAS_ROOT` to `root` and the supervisor role before an attempt.
#[cfg(test)]
pub(crate) struct PeerReviewer {
    _dir: tempfile::TempDir,
    repo: PathBuf,
    pub(crate) root: PathBuf,
    task: cas_types::Task,
}

#[cfg(test)]
impl PeerReviewer {
    pub(crate) fn new(endpoint: &str, mut task: cas_types::Task) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().to_path_buf();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git").current_dir(&repo).args(args).output().unwrap();
            assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        git(&["init", "-q", "-b", "main"]);
        git(&["config", "user.name", "Fixture"]);
        git(&["config", "user.email", "fixture@example.invalid"]);
        std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "base"]);
        git(&["checkout", "-q", "-b", "factory/w"]);
        std::fs::write(repo.join("work.txt"), "delivery\n").unwrap();
        git(&["add", "."]);
        git(&["commit", "-q", "-m", "delivery"]);
        let source = git(&["rev-parse", "HEAD"]);
        git(&["checkout", "-q", "main"]);
        let root = crate::store::init_cas_dir(&repo).unwrap();
        std::fs::write(root.join("config.toml"), "[verification]\nenabled = true\n").unwrap();
        std::fs::write(root.join("cloud.json"),
            serde_json::json!({"endpoint": endpoint, "token": "test-token"}).to_string()).unwrap();
        task.status = cas_types::TaskStatus::AwaitingMerge;
        task.deliverables.factory_branch_anchor = Some(source);
        crate::store::open_task_store(&root).unwrap().add(&task).unwrap();
        Self { _dir: dir, repo, root, task }
    }

    /// One merge attempt: the verification dispatch it held on, or its refusal.
    pub(crate) fn attempt(&self) -> Result<String, String> {
        let core = crate::mcp::CasCore::with_daemon(self.root.clone(), None, None);
        core.set_agent_id_for_testing("owner".into());
        core.open_agent_store().unwrap().register(&cas_types::Agent::new_with_role(
            "owner".into(), "supervisor".into(), cas_types::AgentRole::Supervisor)).unwrap();
        let held = core.prepare_premerge_verification(&self.task, &self.repo, "factory/w", "main");
        assert!(held.is_err(), "an unreviewed delivery cannot merge");
        match cas_store::get_latest_verification_dispatch(&self.root, &self.task.id).unwrap() {
            Some(dispatch) => Ok(dispatch.id),
            None => Err(format!("{:?}", held.err())),
        }
    }

    pub(crate) fn evidence_dir(&self) -> PathBuf {
        evidence_dir(&self.root, &self.task).unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{Mock, MockServer, ResponseTemplate, matchers::{method, path}};

    /// cas-5601: a LEDGER.md published on a peer machine and missing here is
    /// restored by the real pre-merge preparation before its context digest,
    /// so the first attempt reaches dispatch, and a retry fetches nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn peer_evidence_lands_in_the_verifier_evidence_dir_cas_5601() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let server = MockServer::start().await;
        let mut task = cas_types::Task::new("cas-peer".into(), "peer delivery".into());
        let bytes = crate::qa_ledger::scaffold(&task, &[], "0000000").into_bytes();
        let record = PublishedArtifact {
            id: "art-peer1".into(), task_id: "cas-peer".into(), name: "LEDGER.md".into(),
            mime: "text/markdown".into(), size_bytes: bytes.len() as u64,
            sha256: format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(&bytes)),
            cloud_artifact_id: Some("cloud-peer1".into()), status: "committed".into(),
            cloud_url: None, slack_permalink: None, created_at: "2026-11-12T03:00:00Z".into(),
            revocation: None,
        };
        task.notes = format!("[2026-11-12 03:00] Published artifact art-peer1\n{MARKER}{}",
            serde_json::to_string(&record).unwrap());
        Mock::given(method("GET")).and(path("/api/artifacts/cloud-peer1/url"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "artifact_id": "cloud-peer1", "url": format!("{}/blob", server.uri()),
                "size_bytes": bytes.len()})))
            .expect(1).mount(&server).await;
        Mock::given(method("GET")).and(path("/blob"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.clone()))
            .expect(1).mount(&server).await;
        let reviewer = PeerReviewer::new(&server.uri(), task);
        env.set("CAS_ROOT", &reviewer.root);
        env.set("CAS_AGENT_ROLE", "supervisor");

        let (first, second, evidence) = tokio::task::spawn_blocking(move || {
            (reviewer.attempt(), reviewer.attempt(), reviewer.evidence_dir())
        }).await.unwrap();
        let first = first.expect("first attempt reaches dispatch");
        assert_eq!(Ok(first), second, "the retry holds on the same dispatch");
        assert_eq!(std::fs::read(evidence.join("LEDGER.md")).unwrap(), bytes);
    }
}
