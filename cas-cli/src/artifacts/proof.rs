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

/// Local records and synced references cannot establish an exhaustive Cloud
/// inventory. Keep that boundary explicit even when this receiver knows none.
#[derive(serde::Serialize)]
pub struct ArtifactListing {
    pub task_id: String,
    pub state: &'static str,
    pub artifacts: Vec<PublishedArtifact>,
    pub warnings: Vec<&'static str>,
}

pub fn list_task_metadata(cas_root: &Path, task_id: &str) -> anyhow::Result<ArtifactListing> {
    let store = SqliteArtifactStore::open(cas_root)?;
    let mut records = store.list_for_task(task_id)?.into_iter()
        .map(|record| (record.id.clone(), record))
        .collect::<std::collections::BTreeMap<_, _>>();
    let mut warnings = std::collections::BTreeSet::new();
    let task = crate::store::open_task_store_local(cas_root)
        .and_then(|tasks| tasks.get(task_id).map_err(Into::into));
    match task {
        Ok(task) => {
            for json in task.notes.lines().filter_map(|line| line.strip_prefix(MARKER)) {
                let Ok(record) = serde_json::from_str::<PublishedArtifact>(json) else {
                    warnings.insert("invalid_publication_reference");
                    continue;
                };
                // A local revocation remains authoritative over an older note.
                if records.get(&record.id).is_some_and(|local| local.revocation.is_some()) {
                    continue;
                }
                if validate(&record, task_id).is_err() {
                    warnings.insert("invalid_publication_reference");
                } else if let Some(local) = records.get(&record.id) {
                    if !same_artifact(local, &record) {
                        warnings.insert("conflicting_publication_reference");
                    }
                } else {
                    records.insert(record.id.clone(), record);
                }
            }
        }
        Err(crate::error::CasError::StoreErr(cas_store::StoreError::TaskNotFound(_))) => {}
        Err(_) => { warnings.insert("task_references_unavailable"); }
    }
    Ok(ArtifactListing {
        task_id: task_id.into(), state: "partial",
        artifacts: records.into_values().collect(), warnings: warnings.into_iter().collect(),
    })
}

pub fn committed_task_artifacts(cas_root: &Path, task_id: &str) -> anyhow::Result<Vec<PublishedArtifact>> {
    Ok(list_task_metadata(cas_root, task_id)?.artifacts.into_iter()
        .filter(|artifact| artifact.status == "committed" && artifact.cloud_artifact_id.is_some())
        .collect())
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
            reason: "publication_reference_missing: no local artifact record or synced task publication reference matches this ID. Sync this receiver and confirm that the publisher's publication note reached Cloud. If Cloud lacks that reference, the publisher must sync it before this receiver can recover it.",
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
    // GH #2317: a concurrent restore of the same id may have recorded it while
    // the bytes downloaded. Its row is this restore's outcome, not a conflict.
    if existing.is_none() && store.get(&artifact.id)?.is_none() {
        if let Err(error) = store.record_local(&NewArtifact {
            id: artifact.id.clone(),
            task_id: artifact.task_id.clone(),
            name: artifact.name.clone(),
            mime: artifact.mime.clone(),
            size_bytes: artifact.size_bytes,
            sha256: artifact.sha256.clone(),
        }) {
            anyhow::ensure!(store.get(&artifact.id)?.is_some_and(|row| same_artifact(&row, &artifact)), error);
            return Ok(destination);
        }
        store.mark_uploaded(&artifact.id, cloud_id)?;
        store.mark_committed(&artifact.id, None)?;
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
const EVIDENCE_REPORT: &str = "evidence-sync-report.json";
const EVIDENCE_SAMPLE: usize = 20;
const EVIDENCE_NOTE_BYTES: usize = 16 * 1024;

fn short_evidence_text(text: &str, bytes: usize) -> String {
    let clean: String = text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    if clean.len() <= bytes { return clean; }
    let end = clean.char_indices().map(|(i, _)| i).take_while(|i| *i <= bytes - 3).last().unwrap_or(0);
    format!("{}...", &clean[..end])
}

#[derive(serde::Serialize, serde::Deserialize)]
struct EvidenceReportRef {
    path: String,
    artifact_id: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct EvidenceReport {
    manifest: EvidenceSync,
    skipped: Vec<String>,
    unpublished: Vec<String>,
    excluded: Vec<String>,
}

fn evidence_summary(report: &EvidenceReport) -> String {
    let mut summary = format!("Files: {}; optional not published: {}; required not published: {}; excluded trees: {}. Full report: {EVIDENCE_REPORT}",
        report.manifest.files.len(), report.skipped.len(), report.unpublished.len(), report.excluded.len());
    // One shared sample budget, including errors and excluded directories.
    for item in report.unpublished.iter().chain(&report.skipped).chain(&report.excluded)
        .chain(report.manifest.files.iter().filter(|_| report.unpublished.is_empty()).map(|file| &file.name)).take(EVIDENCE_SAMPLE)
    {
        summary.push_str("\n- ");
        summary.push_str(&short_evidence_text(item, 256));
    }
    summary
}

/// cas-8018 (GH #2268): park publication is bounded so a close never waits on
/// a slow Cloud. Each upload request gets at most [`PUBLISH_REQUEST_TIMEOUT`]
/// or the remaining shared budget, whichever is shorter. A failed
/// upload is retried once while the budget lasts, and past the budget the
/// remaining files count as failed. Files committed earlier are reused, so a
/// retried close resumes where the last one stopped.
const PUBLISH_BUDGET: std::time::Duration = std::time::Duration::from_secs(20);
const PUBLISH_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// cas-fd1c (GH #2155, #2203, #2212): the task-relative paths a LEDGER.md's
/// rows cite, plus the ledger itself; `None` without a ledger. Each evidence
/// cell is one path, resolved exactly as the ledger check does
/// ([`crate::qa_ledger::evidence_cell_path`]): relative, absolute inside the
/// directory, or another machine's artifacts path. A cited directory covers the
/// files beneath it.
fn ledger_citations(dir: &Path) -> Option<std::collections::BTreeSet<String>> {
    let body = std::fs::read_to_string(dir.join("LEDGER.md")).ok()?;
    let root = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
    let mut names: std::collections::BTreeSet<String> = crate::qa_ledger::parse(&body, 0).rows.into_iter()
        .filter_map(|row| crate::qa_ledger::evidence_cell_path(&root, &row.evidence_path))
        .filter_map(|cited| cited.strip_prefix(&root).ok()?.to_str().map(|name| name.replace(std::path::MAIN_SEPARATOR, "/")))
        .filter(|name| !name.is_empty())
        .collect();
    names.insert("LEDGER.md".into());
    Some(names)
}

/// Evidence trees Cassy's own gates read from the task evidence directory,
/// so a peer reviewer needs them whole: the QA bundle, whose files resolve
/// relative to `qa/` (`qa_evidence::resolve_bundle_file`), and terminal-qa
/// receipts, found by walking `terminal-qa/` (`validate_terminal_qa`).
const VERIFIER_TREES: [&str; 2] = ["qa/", "terminal-qa/"];

/// What a routed build's park published: its evidence directory and the
/// exact bytes the reviewing machine must hold before review (cas-e571).
#[derive(serde::Serialize, serde::Deserialize)]
struct EvidenceSync {
    machine_id: String,
    root: String,
    files: Vec<SyncedFile>,
    /// A routed build's manifest must arrive byte-identical before review.
    /// An ordinary park's manifest restores what it can and never refuses
    /// (cas-dae1: no new admission gate). Manifests written before this
    /// field existed were all routed.
    #[serde(default = "strict_by_default")]
    strict: bool,
    /// Large manifests live in a verified artifact, rather than a synced row.
    /// Absent on legacy and small inline manifests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    report: Option<EvidenceReportRef>,
}

fn strict_by_default() -> bool { true }

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

/// The top-level name `fetch_task` gives a downloaded artifact.
fn fetched_copy(name: &str) -> bool {
    name.strip_prefix("art-").and_then(|rest| rest.get(..9))
        .is_some_and(|id| id.ends_with('-') && id[..8].bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// Every regular file under `dir`, recursively, by task-relative path.
fn evidence_files(dir: &Path) -> std::io::Result<(Vec<(String, PathBuf)>, Vec<String>)> {
    let mut files = Vec::new();
    let mut excluded = Vec::new();
    let walker = walkdir::WalkDir::new(dir).min_depth(1).sort_by_file_name().into_iter().filter_entry(|entry| {
        if entry.file_type().is_dir() && matches!(entry.file_name().to_str(), Some("node_modules" | ".git" | "vendor" | ".venv")) {
            excluded.push(entry.path().strip_prefix(dir).unwrap_or(entry.path()).display().to_string());
            return false;
        }
        true
    });
    for entry in walker {
        let entry = entry.map_err(std::io::Error::other)?;
        if !entry.file_type().is_file() {
            continue;
        }
        let relative = entry.path().strip_prefix(dir).map_err(std::io::Error::other)?;
        // Never recursively publish the previous report or a fetched copy.
        // A fetched copy (`art-<id>-<name>`, `fetch_task`) is already an
        // artifact; republishing it nests a new copy on every park (cas-7c35).
        if relative.components().count() == 1 && relative.to_str().is_some_and(|name|
            name == EVIDENCE_REPORT || fetched_copy(name))
        { continue; }
        let name = relative.to_str().filter(|name| safe_relative(name)).ok_or_else(|| {
            std::io::Error::other(format!("{} has no portable task-relative name", entry.path().display()))
        })?;
        files.push((name.replace(std::path::MAIN_SEPARATOR, "/"), entry.path().to_path_buf()));
    }
    Ok((files, excluded))
}

fn evidence_dir(cas_root: &Path, task: &cas_types::Task) -> anyhow::Result<PathBuf> {
    crate::mcp::tools::core::task::lifecycle::qa_evidence_gate::task_evidence_dir(cas_root, task)
        .map_err(anyhow::Error::msg)
}

/// The path a close-reason token cites. Backtick-quoted text is the path
/// verbatim. Prose can wrap an unquoted one, "(target/x.log)." or
/// "…/LEDGER.md.", so trailing prose punctuation is dropped one character at a
/// time and the longest form that exists wins: a literal file `x.log!` is never
/// read as its sibling `x.log`. When no form exists the fully stripped path is
/// the one reported missing (cas-df37).
fn cited_token(token: &str, quoted: bool, exists: impl Fn(&str) -> bool) -> &str {
    fn value(text: &str) -> &str {
        text.split_once('=').map_or(text, |(_, value)| value)
    }
    if quoted {
        return value(token);
    }
    let wrapper = |c: char| matches!(c, '(' | ')' | '[' | ']' | ',' | ';' | '"' | '\'');
    let mut cited = token.trim_start_matches(wrapper);
    let stripped = cited.trim_end_matches(|c: char| wrapper(c) || matches!(c, '.' | ':' | '!' | '?')).len();
    while cited.len() > stripped && !exists(value(cited)) {
        cited = &cited[..cited.len() - 1]; // only ASCII punctuation lies past `stripped`
    }
    value(cited)
}

/// Directory words prose uses after `target/`, `qa/` or an artifacts path.
const DIRECTORY_WORDS: [&str; 12] =
    ["target", "owner", "dist", "build", "out", "logs", "debug", "release", "qa", "terminal-qa", "cited", "artifacts"];

/// GH #2303, #2305, #2337, #2371, cas-4b90: whether a missing close-reason
/// token cites a concrete evidence file, which a peer then requires. Prose
/// ("merged target", "writes under target/ci-impact", "target/owner"),
/// directories and illustrative `{a,b}.log` or `<task>/x` forms never do.
/// It must lie strictly beneath an evidence root, without `..`, and either
/// have a file extension or, outside the build tree, be a multi-segment path
/// whose leaf is not a directory word.
fn cites_file(token: &str, path: &Path, build_tree: &Path, evidence_dir: &Path) -> bool {
    if token.ends_with('/') || token.contains(['*', '?', '{', '}', '<', '>', '|', '\u{2026}']) || token.contains("...")
        || Path::new(token).components().any(|part| part == std::path::Component::ParentDir)
        || path == build_tree || evidence_dir.starts_with(path)
    {
        return false;
    }
    let Some(leaf) = path.file_name().and_then(|leaf| leaf.to_str()) else { return false };
    Path::new(leaf).extension().is_some()
        || (token.contains('/') && !path.starts_with(build_tree) && !DIRECTORY_WORDS.contains(&leaf))
}

/// Publish close-reason citations for a reviewer on either machine. Routed
/// builds additionally require their ledger evidence and strict restoration;
/// uncited ordinary QA captures remain best effort (cas-dae1, cas-2de9).
pub(crate) fn publish_routed_evidence(
    cas_root: &Path,
    task: &cas_types::Task,
    proof_repo: &Path,
) -> Result<(), String> {
    let source = crate::factory_build_routing::routed_from(task);
    let machine_id = cas_types::Agent::get_or_generate_machine_id();
    let routed = source.as_deref().is_some_and(|source| source != machine_id)
        || crate::cloud::peer_mailbox::train_parks::host_worker(cas_root,task).is_some();
    let dir = evidence_dir(cas_root, task).map_err(|error| error.to_string())?;
    let fail = |detail: String| format!(
        "EVIDENCE PUBLICATION INCOMPLETE: {detail}. Task {} has local evidence in {}; publish it before a peer reviews the delivery.",
        task.id, short_evidence_text(&dir.display().to_string(), 1024));
    let fail = |detail: String| fail(short_evidence_text(&detail, 8192));
    let (mut paths, excluded) = if dir.exists() {
        evidence_files(&dir).map_err(|error| fail(error.to_string()))?
    } else { (Vec::new(), Vec::new()) };
    let mut unpublished = Vec::new();
    let mut cited_names = std::collections::BTreeSet::new();
    // A close can also cite a capped runner log in the worker checkout.
    // Publish only evidence locations, never arbitrary source mentions.
    for (index, segment) in task.deliverables.parked_close_reason.as_deref().unwrap_or_default().split('`').enumerate() {
        let tokens = if index % 2 == 1 { vec![segment] } else { segment.split_whitespace().collect() };
        for token in tokens {
            let resolve = |text: &str| {
                let expanded = crate::qa_evidence::expand_home(text);
                if expanded.is_absolute() { expanded } else { proof_repo.join(expanded) }
            };
            let in_evidence = |text: &str, path: &Path| path.starts_with(proof_repo.join("target"))
                || path.starts_with(&dir)
                || text.contains("/.cas/artifacts/") || text.starts_with(".cas/artifacts/");
            let token = cited_token(token, index % 2 == 1, |text| {
                let path = resolve(text);
                in_evidence(text, &path) && path.exists()
            });
            let path = resolve(token);
            if !in_evidence(token, &path) { continue; }
            // Directory mentions are context, but a missing cited file must
            // not silently disappear from the peer's proof (cas-dae1).
            if path.is_dir() { continue; }
            if !path.is_file() {
                let missing = format!("{}: missing or not a regular file", path.display());
                if cites_file(token, &path, &proof_repo.join("target"), &dir) && !unpublished.contains(&missing) {
                    unpublished.push(missing);
                }
            } else if let Some((name, _)) = paths.iter().find(|(_, existing)| *existing == path) {
                // A direct close citation makes an already scanned copy
                // required even when the ledger does not list it.
                cited_names.insert(name.clone());
            } else {
                let leaf = path.file_name().and_then(|name| name.to_str()).unwrap_or("proof.log");
                let name = format!("cited/{leaf}");
                cited_names.insert(name.clone());
                paths.push((name, path));
            }
        }
    }
    if paths.is_empty() && unpublished.is_empty() && excluded.is_empty() {
        return Ok(());
    }
    let store = SqliteArtifactStore::open(cas_root).map_err(|error| fail(error.to_string()))?;
    let config = crate::config::Config::load(cas_root).unwrap_or_default();
    let base = crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    // A park publishes declared evidence only: top-level files, close-reason
    // citations, ledger citations and the trees Cassy's verifier reads
    // (cas-e571). Any other nested file nothing cites (a copied
    // source checkout, raw captures already bundled) is listed, never uploaded.
    let ledger = ledger_citations(&dir);
    let ledger_cites = |name: &str| ledger.as_ref().is_some_and(|cited| cited.iter().any(|entry| {
        name.strip_prefix(entry.as_str()).is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    }));
    let declared = |name: &str| !name.contains('/') || cited_names.contains(name) || ledger_cites(name)
        || VERIFIER_TREES.iter().any(|tree| name.starts_with(tree));
    // Routed builds require their ledger's citations, or without a ledger
    // every declared file.
    let required = |name: &str| cited_names.contains(name)
        || (routed && declared(name) && (ledger.is_none() || ledger_cites(name)));
    // A stalled uncited copy must not spend the budget needed by the ledger.
    paths.sort_by_key(|(name, _)| !required(name));
    let deadline = std::time::Instant::now() + PUBLISH_BUDGET;
    let context = super::PublishContext {
        store: &store,
        roots: PublishRoots::new(cas_root, &base, &task.id),
        task_id: task.id.clone(),
        cloud: super::cloud_client(cas_root).map(|client| client
            .with_timeout(PUBLISH_REQUEST_TIMEOUT).with_deadline(deadline)),
    };
    let mut published = store.list_for_task(&task.id).map_err(|error| fail(error.to_string()))?;
    // cas-1797 (GH #2267): proof another machine already committed for this
    // task reaches here as synced notes. Its exact bytes are already fetchable
    // by the peer, so a transient upload failure must not refuse the park. A
    // conflicting or locally revoked record is never reused.
    for json in task.notes.lines().filter_map(|line| line.strip_prefix(MARKER)) {
        let Ok(record) = serde_json::from_str::<PublishedArtifact>(json) else { continue };
        if published.iter().any(|known| known.id == record.id)
            || !from_notes(&task.notes, &record.id, &task.id).is_ok_and(|found| found.is_some())
            || store.get(&record.id).ok().flatten().is_some_and(|local| super::require_available(&local).is_err())
        { continue; }
        published.push(record);
    }
    let upload = |path: &Path| -> Result<String, String> {
        let outcome = super::publish(&context, path).map_err(|error| error.to_string())?;
        if outcome.disposition.storage_disposition() != super::StorageDisposition::Committed {
            return Err(outcome.disposition.summary());
        }
        Ok(outcome.artifact.id)
    };
    let mut skipped = Vec::new();
    let mut files = Vec::new();
    for (name, path) in paths {
        let leaf = path.file_name().and_then(|leaf| leaf.to_str()).unwrap_or_default();
        // An uncited, non-empty file nobody has published is listed, never read
        // or hashed. An empty file costs nothing and stays in the manifest
        // (cas-591f: every file is restorable, large trees stay bounded).
        let empty = std::fs::metadata(&path).is_ok_and(|meta| meta.len() == 0);
        if !empty && !declared(&name) && !published.iter().any(|record| record.name == leaf) {
            skipped.push(format!("{name} (uncited; not published)"));
            continue;
        }
        let (sha256, size) = digest_file(&path).map_err(|error| fail(error.to_string()))?;
        if size == 0 {
            files.push(SyncedFile { name, sha256, artifact_id: None });
            continue;
        }
        let reused = published.iter().find(|record| {
            record.name == leaf && record.sha256 == sha256 && validate(record, &task.id).is_ok()
        });
        let artifact_id = match reused {
            Some(record) => record.id.clone(),
            None if !declared(&name) => {
                skipped.push(format!("{name} (uncited; not published)"));
                continue;
            }
            None => {
                let within = || std::time::Instant::now() < deadline;
                let budget = || format!("not published within the {} s park budget; retry close", PUBLISH_BUDGET.as_secs());
                let result = if within() {
                    upload(&path).or_else(|first| if within() { upload(&path) } else { Err(first) })
                } else {
                    Err(budget())
                };
                match result {
                    Ok(id) => id,
                    Err(reason) if required(&name) => {
                        unpublished.push(format!("{}: {reason}", path.display()));
                        continue;
                    }
                    Err(reason) => {
                        skipped.push(format!("{name} ({reason})"));
                        continue;
                    }
                }
            }
        };
        files.push(SyncedFile { name, sha256, artifact_id: Some(artifact_id) });
    }
    // Every park records where each evidence file sits under the task evidence
    // directory, so the merge owner can restore nested captures at the paths
    // the ledger cites (GH #2079). Only a routed build makes it strict.
    let audience = if routed { source.unwrap_or_else(|| "the merge owner".into()) } else { "the merge owner".into() };
    let manifest = EvidenceSync { machine_id, root: dir.display().to_string(), files, strict: routed, report: None };
    let report = EvidenceReport { manifest, skipped, unpublished, excluded };
    std::fs::create_dir_all(&dir).map_err(|error| fail(error.to_string()))?;
    let report_path = dir.join(EVIDENCE_REPORT);
    std::fs::write(&report_path, serde_json::to_vec(&report).map_err(|error| fail(error.to_string()))?)
        .map_err(|error| fail(error.to_string()))?;
    let summary = evidence_summary(&report);
    if !report.unpublished.is_empty() { return Err(fail(summary)); }
    let inline = serde_json::to_string(&report.manifest).map_err(|error| fail(error.to_string()))?;
    let manifest_note = if inline.len() <= 4096 && report.manifest.files.len() <= EVIDENCE_SAMPLE {
        inline
    } else {
        let artifact_id = match upload(&report_path) {
            Ok(id) => Some(id),
            Err(error) if routed => return Err(fail(format!("{summary}\nManifest publication: {error}"))),
            Err(_) => None, // Ordinary evidence remains best effort.
        };
        serde_json::to_string(&EvidenceSync {
            machine_id: short_evidence_text(&report.manifest.machine_id, 128),
            root: short_evidence_text(&report.manifest.root, 512), files: Vec::new(), strict: routed,
            report: Some(EvidenceReportRef { path: EVIDENCE_REPORT.into(), artifact_id }),
        }).map_err(|error| fail(error.to_string()))?
    };
    let note = format!(
        "[{}] Evidence synced for review on {audience}\n{EVIDENCE_SYNC}{}",
        chrono::Utc::now().format("%Y-%m-%d %H:%M"),
        manifest_note,
        audience = short_evidence_text(&audience, 256),
    );
    let note = format!("{note}\n{summary}");
    if note.len() >= EVIDENCE_NOTE_BYTES { return Err(fail("Evidence summary exceeds its byte budget".into())); }
    let tasks = crate::store::open_task_store(cas_root).map_err(|error| fail(error.to_string()))?;
    // GH #2322: the newest manifest is the only one read, so a retried park
    // replaces its previous evidence-sync note rather than appending another.
    tasks.append_trusted_note(&task.id, &note).map_err(|error| fail(error.to_string()))?;
    Ok(())
}

/// Synced publication times order revisions. A receiver's artifact-store time
/// records its import, so it cannot establish which evidence revision is newer.
fn newer_local_publication(
    store: &SqliteArtifactStore,
    task: &cas_types::Task,
    file: &SyncedFile,
    local_sha: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let (Some(id), Some(local_sha)) = (&file.artifact_id, local_sha) else { return Ok(None) };
    let Some(manifest) = from_notes(&task.notes, id, &task.id)? else { return Ok(None) };
    anyhow::ensure!(manifest.sha256 == file.sha256, "artifact {id} has other bytes");
    let Ok(published) = chrono::DateTime::parse_from_rfc3339(&manifest.created_at) else { return Ok(None) };
    for json in task.notes.lines().filter_map(|line| line.strip_prefix(MARKER)) {
        let Ok(record) = serde_json::from_str::<PublishedArtifact>(json) else { continue };
        if record.name != manifest.name || record.sha256 != local_sha
            || validate(&record, &task.id).is_err()
            || !chrono::DateTime::parse_from_rfc3339(&record.created_at).is_ok_and(|time| time > published)
        { continue; }
        // Keep the same conflict/revocation checks as a fetch, even when no
        // download is needed for these already-present bytes.
        from_notes(&task.notes, &record.id, &task.id)?;
        if let Some(local) = store.get(&record.id)? {
            super::require_available(&local)?;
            anyhow::ensure!(same_artifact(&local, &record), "cached artifact {} has a different identity", record.id);
        }
        return Ok(Some(record.id));
    }
    Ok(None)
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
        if sync.strict && let Some(id) = &file.artifact_id {
            let committed = (|| -> anyhow::Result<()> {
                let artifact = from_notes(&task.notes, id, &task.id)?.ok_or_else(|| anyhow::anyhow!("no committed artifact record {id}"))?;
                anyhow::ensure!(artifact.sha256 == file.sha256, "artifact {id} has other bytes");
                if let Some(local) = store.get(id)? {
                    super::require_available(&local)?;
                    anyhow::ensure!(same_artifact(&local, &artifact), "cached artifact {id} has a different identity");
                }
                Ok(())
            })();
            if let Err(error) = committed { missing.push(format!("{} ({error})", file.name)); continue; }
        }
        let local_sha = digest_file(&dir.join(&file.name)).ok().map(|(sha256, _)| sha256);
        if local_sha.as_deref() == Some(file.sha256.as_str()) {
            continue;
        }
        match newer_local_publication(&store, task, file, local_sha.as_deref()) {
            Ok(Some(id)) => {
                if sync.strict {
                    missing.push(format!("{} (newer local artifact {id}; re-park with its updated manifest)", file.name));
                }
                continue;
            }
            Err(error) => { missing.push(format!("{} ({error})", file.name)); continue; }
            Ok(None) => {}
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
    let sealed = task.notes.lines().any(|line| line.starts_with(crate::cloud::peer_mailbox::train_parks::NOTE));
    if sealed && manifest.is_none() { return Err("Remote park has no valid complete evidence manifest".into()); }
    if let Some(mut sync) = manifest {
        sync.strict |= sealed;
        let restored = (|| {
            if let Some(reference) = &sync.report {
                let loaded = (|| -> anyhow::Result<EvidenceSync> {
                    anyhow::ensure!(reference.path == EVIDENCE_REPORT, "invalid evidence report path");
                    let path = if let Some(id) = &reference.artifact_id {
                        // Use the same authenticated task snapshot as the
                        // manifest, rather than re-reading a stale local row.
                        let artifact = from_notes(&task.notes, id, &task.id)?
                            .ok_or_else(|| anyhow::anyhow!("no synced artifact record {id}"))?;
                        let store = SqliteArtifactStore::open(cas_root)?;
                        let config = crate::config::Config::load(cas_root)?;
                        let base = crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
                        let roots = PublishRoots::new(cas_root, &base, &task.id);
                        fetch(&store, super::cloud_client(cas_root).as_ref(), &roots, artifact,
                            &format!("{id}-{EVIDENCE_REPORT}"))?
                    } else {
                        anyhow::ensure!(!sync.strict, "strict evidence report is not published");
                        evidence_dir(cas_root, task)?.join(&reference.path)
                    };
                    let report: EvidenceReport = serde_json::from_slice(&std::fs::read(path)?)?;
                    anyhow::ensure!(report.manifest.report.is_none(), "nested evidence report");
                    Ok(report.manifest)
                })().map_err(|error| format!("EVIDENCE NOT SYNCED: report {}: {error}", reference.path))?;
                let strict = sync.strict;
                sync = loaded;
                sync.strict |= strict;
            }
            materialize_manifest(cas_root, task, &sync)
        })();
        if sync.strict {
            return restored;
        }
        if let Err(error) = restored {
            tracing::warn!(task_id = %task.id, %error, "peer evidence manifest not fully restored");
        }
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

    /// GH #2292: file-count growth belongs in an artifact, never a task row.
    /// Both the full manifest and optional failures retain their detail while
    /// the synced summary stays bounded and the peer restores the last file.
    #[tokio::test(flavor = "multi_thread")]
    async fn evidence_sync_notes_stay_bounded_over_5000_files_cas_591f() {
        use wiremock::matchers::body_partial_json;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        for optional_failures in [false, true] {
            let server = MockServer::start().await;
            let uploaded = std::sync::Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
            Mock::given(method("POST")).and(path("/api/artifacts/begin"))
                .and(body_partial_json(serde_json::json!({"name":"evidence-sync-report.json"})))
                .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                    "artifact_id":"cloud-report", "upload_url":format!("{}/report",server.uri()),
                    "required_headers":{},
                }))).mount(&server).await;
            let captured = uploaded.clone();
            Mock::given(method("PUT")).and(path("/report"))
                .respond_with(move |request: &wiremock::Request| {
                    *captured.lock().unwrap() = request.body.clone();
                    ResponseTemplate::new(200)
                }).mount(&server).await;
            Mock::given(method("POST")).and(path("/api/artifacts/cloud-report/complete"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"url":null})))
                .mount(&server).await;
            Mock::given(method("GET")).and(path("/api/artifacts/cloud-report/url"))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "artifact_id":"cloud-report", "url":format!("{}/report",server.uri()),
                }))).mount(&server).await;
            let captured = uploaded.clone();
            Mock::given(method("GET")).and(path("/report"))
                .respond_with(move |_: &wiremock::Request| {
                    ResponseTemplate::new(200).set_body_bytes(captured.lock().unwrap().clone())
                }).mount(&server).await;
            let mut task = cas_types::Task::new("cas-591f-fixture".into(), "large evidence tree".into());
            if !optional_failures {
                task.notes = format!("BUILD_ROUTE: {}", serde_json::json!({
                    "source_machine_id":"peer-fixture", "key":"large-evidence", "sender_agent_id":"sup",
                    "recipient_agent_id":"worker", "recipient":"worker", "free_slots":1, "load_per_cpu":null,
                }));
            }
            let parking = PeerReviewer::new(&server.uri(), task);
            env.set("CAS_ROOT", &parking.root);
            let dir = parking.evidence_dir();
            std::fs::create_dir_all(dir.join("captures")).unwrap();
            std::fs::create_dir_all(dir.join("srt/node_modules/dependency")).unwrap();
            for index in 0..5000 {
                std::fs::write(dir.join(format!("captures/{index:04}.txt")), b"").unwrap();
            }
            for index in 0..100 {
                std::fs::write(dir.join(format!("srt/node_modules/dependency/{index}.js")), b"").unwrap();
            }
            if optional_failures {
                for index in 0..96 {
                    std::fs::write(dir.join(format!("optional-{index:03}-{}.log", "x".repeat(180))), b"not uploaded").unwrap();
                }
            }
            let (parked, report) = tokio::task::spawn_blocking(move || {
                publish_routed_evidence(&parking.root, &parking.task, &parking.repo).unwrap();
                let parked = crate::store::open_task_store(&parking.root).unwrap().get(&parking.task.id).unwrap();
                assert!(parked.notes.len() < 16 * 1024,
                    "evidence sync appended {} bytes for 5,000 files", parked.notes.len());
                let report: serde_json::Value = serde_json::from_slice(
                    &std::fs::read(dir.join("evidence-sync-report.json")).unwrap()).unwrap();
                (parked, report)
            }).await.unwrap();
            assert_eq!(report["manifest"]["files"].as_array().unwrap().len(), 5000);
            assert_eq!(report["skipped"].as_array().unwrap().len(), if optional_failures { 96 } else { 0 });
            assert_eq!(report["excluded"].as_array().unwrap().len(), 1);
            let owner = PeerReviewer::new(&server.uri(), parked);
            env.set("CAS_ROOT", &owner.root);
            tokio::task::spawn_blocking(move || {
                materialize_peer_evidence(&owner.root, &owner.task).unwrap();
                assert_eq!(std::fs::read(owner.evidence_dir().join("captures/4999.txt")).unwrap(), b"");
                assert!(!owner.evidence_dir().join("srt/node_modules").exists());
            }).await.unwrap();
        }
    }

    /// cas-5f96: an older park must not revert a subsequently published ledger.
    /// A sealed park refuses that revision; a newer park still replaces old bytes.
    #[tokio::test(flavor = "multi_thread")]
    async fn newer_published_ledger_survives_stale_manifest_cas_5f96() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let server = MockServer::start().await;
        let task = cas_types::Task::new("cas-5f96-fixture".into(), "ledger revisions".into());
        let old_bytes = b"build e5d559d1d4\n";
        let new_bytes = b"build 4e6f0d2695\n";
        let mut records = Vec::new();
        for (id, bytes, created_at) in [
            ("old", old_bytes.as_slice(), "2026-11-20T20:00:00-05:00"),
            ("new", new_bytes.as_slice(), "2026-11-21T01:30:00Z"),
        ] {
            let record = PublishedArtifact {
                id: format!("art-{id}"), task_id: task.id.clone(), name: "LEDGER.md".into(),
                mime: "text/markdown".into(), size_bytes: bytes.len() as u64,
                sha256: format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(bytes)),
                cloud_artifact_id: Some(format!("cloud-{id}")), status: "committed".into(),
                cloud_url: None, slack_permalink: None, created_at: created_at.into(), revocation: None,
            };
            Mock::given(method("GET")).and(path(format!("/api/artifacts/cloud-{id}/url")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "artifact_id": format!("cloud-{id}"), "url": format!("{}/blob/{id}", server.uri()),
                    "size_bytes": bytes.len()})))
                .expect(2).mount(&server).await;
            Mock::given(method("GET")).and(path(format!("/blob/{id}")))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.to_vec()))
                .expect(2).mount(&server).await;
            records.push(record);
        }
        let owner = PeerReviewer::new(&server.uri(), task);
        env.set("CAS_ROOT", &owner.root);
        tokio::task::spawn_blocking(move || {
            let ledger = owner.evidence_dir().join("LEDGER.md");
            std::fs::create_dir_all(ledger.parent().unwrap()).unwrap();
            for strict in [false, true] {
                for (local, manifest_index, published_new, expected) in [
                    (new_bytes.as_slice(), 0, true, new_bytes.as_slice()),
                    (old_bytes.as_slice(), 1, true, new_bytes.as_slice()),
                    (new_bytes.as_slice(), 0, false, old_bytes.as_slice()),
                ] {
                    std::fs::write(&ledger, local).unwrap();
                    let artifact = &records[manifest_index];
                    let mut task = owner.task.clone();
                    // Repeated older proof notes must not become newer revisions.
                    task.notes = format!("{MARKER}{}\n{MARKER}{}\n{EVIDENCE_SYNC}{}",
                        serde_json::to_string(&records[usize::from(published_new)]).unwrap(),
                        serde_json::to_string(&records[0]).unwrap(),
                        serde_json::to_string(&EvidenceSync {
                            machine_id: "publisher".into(), root: "/publisher/evidence".into(), strict, report: None,
                            files: vec![SyncedFile { name: "LEDGER.md".into(),
                                sha256: artifact.sha256.clone(), artifact_id: Some(artifact.id.clone()) }],
                        }).unwrap());
                    let result = materialize_peer_evidence(&owner.root, &task);
                    assert_eq!(std::fs::read(&ledger).unwrap(), expected,
                        "strict={strict}, manifest={manifest_index}, published_new={published_new}");
                    assert_eq!(result.is_err(), strict && manifest_index == 0 && published_new);
                    if result.is_err() {
                        assert!(result.unwrap_err().contains(&records[1].id));
                    }
                }
            }
        }).await.unwrap();
    }

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
    /// cas-dae1: ordinary parked deliveries carry their cited worktree logs,
    /// without requiring a routed-build marker or a manual publish round trip.
    /// cas-7c35: replays the evidence-family close reasons on a checkout
    /// without `target` (GH #2371's pnpm repo). Only a concrete cited file is
    /// required; prose, directories and illustrative paths never are. A
    /// fetched `art-<id>-` copy is never republished, and every successful
    /// retry replaces its evidence-sync note (GH #2322).
    #[tokio::test(flavor = "multi_thread")]
    async fn park_requires_only_cited_evidence_files_cas_7c35() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let server = MockServer::start().await; // no mocks: any upload fails
        let task = cas_types::Task::new("cas-7c35-fixture".into(), "evidence family".into());
        let owner = PeerReviewer::new(&server.uri(), task);
        env.set("CAS_ROOT", &owner.root);
        let dir = owner.evidence_dir();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("HANDOFF.md"), b"").unwrap();
        std::fs::write(dir.join("art-1234abcd-art-5678abcd-HANDOFF.md"), b"fetched copy").unwrap();
        let d = dir.display();
        let cases: Vec<(&str, String, usize)> = vec![
            ("GH #2303", "PASS abc. Merged epic target 1234 into returned delivery 5678; both verified ancestors".into(), 0),
            ("GH #2305", format!("merged target preserving both changelog entries; logs {d}/{{a,b}}.log"), 0),
            ("GH #2337", "target already contains it; held on target.".into(), 0),
            ("cas-4b90", "Safety: writes under target/ci-impact only.".into(), 0),
            ("cas-6456", "Routed by the target/owner map (target/owner).".into(), 0),
            ("placeholder", format!("see {d}/<run>.log and {d}/\u{2026}/x.log and {d}/../escape.log"), 0),
            ("evidence dir", format!("evidence in {d} and {d}/"), 0),
            ("cited log", "Log target/runner.log, again (target/runner.log).".into(), 1),
            ("extensionless", format!("receipt {d}/RUN-OUTPUT"), 1),
        ];
        tokio::task::spawn_blocking(move || {
            for (case, reason, required) in cases {
                let mut parking = owner.task.clone();
                parking.deliverables.parked_close_reason = Some(reason);
                let outcome = publish_routed_evidence(&owner.root, &parking, &owner.repo);
                let report: EvidenceReport = serde_json::from_slice(
                    &std::fs::read(dir.join(EVIDENCE_REPORT)).unwrap()).unwrap();
                assert_eq!(report.unpublished.len(), required, "{case}: {:?}", report.unpublished);
                assert_eq!(outcome.is_ok(), required == 0, "{case}: {outcome:?}");
                assert!(report.manifest.files.iter().map(|file| &file.name).chain(&report.skipped)
                    .all(|name| !name.contains("art-1234abcd")), "{case}: fetched copy republished");
            }
            let notes = crate::store::open_task_store(&owner.root).unwrap().get(&owner.task.id).unwrap().notes;
            assert_eq!(notes.lines().filter(|line| line.starts_with(EVIDENCE_SYNC)).count(), 1, "{notes}");
        }).await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn ordinary_park_publishes_cited_worktree_evidence_cas_dae1() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let server = MockServer::start().await;
        // The second file is a literal name ending in prose punctuation, beside
        // a different file of the stripped name that must never be read.
        let cited: [(&str, &[u8]); 2] = [("runner.log", b"test: PASS fixture\n"), ("proof.log!", b"literal\n")];
        for (index, (leaf, bytes)) in cited.iter().enumerate() {
            Mock::given(method("POST")).and(path("/api/artifacts/begin"))
                .and(wiremock::matchers::body_partial_json(serde_json::json!({"name": leaf})))
                .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                    "artifact_id": format!("cloud-{index}"), "upload_url": format!("{}/blob/{index}", server.uri()),
                    "required_headers": {}
                }))).expect(1).mount(&server).await;
            Mock::given(method("PUT")).and(path(format!("/blob/{index}")))
                .and(wiremock::matchers::body_bytes(bytes.to_vec()))
                .respond_with(ResponseTemplate::new(200)).expect(1).mount(&server).await;
            Mock::given(method("POST")).and(path(format!("/api/artifacts/cloud-{index}/complete")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"url": null})))
                .expect(1).mount(&server).await;
        }
        let task = cas_types::Task::new("cas-dae1-fixture".into(), "ordinary park".into());
        let mut owner = PeerReviewer::new(&server.uri(), task);
        env.set("CAS_ROOT", &owner.root);
        let log = owner.repo.join("target/runner.log");
        std::fs::create_dir_all(log.parent().unwrap()).unwrap();
        for (leaf, bytes) in cited.iter().chain([&("proof.log", b"sibling\n" as &[u8])]) {
            std::fs::write(owner.repo.join("target").join(leaf), bytes).unwrap();
        }
        std::fs::create_dir_all(owner.evidence_dir()).unwrap();
        // cas-df37: prose punctuation around a cited path is not part of it,
        // but a quoted path is verbatim and an existing exact path wins.
        owner.task.deliverables.parked_close_reason = Some(format!(
            "VERIFIED (target/runner.log). Log: {}. Also `target/proof.log!` and (target/proof.log!).", log.display()));
        tokio::task::spawn_blocking(move || {
            publish_routed_evidence(&owner.root, &owner.task, &owner.repo).unwrap();
            let records = SqliteArtifactStore::open(&owner.root).unwrap().list_for_task(&owner.task.id).unwrap();
            let mut names: Vec<&str> = records.iter().map(|record| record.name.as_str()).collect();
            names.sort_unstable();
            assert_eq!(names, ["proof.log!", "runner.log"]);
            for record in &records {
                validate(record, &owner.task.id).unwrap();
                assert_eq!(record.sha256, digest_file(&owner.repo.join("target").join(&record.name)).unwrap().0);
            }
            let task = crate::store::open_task_store(&owner.root).unwrap().get(&owner.task.id).unwrap();
            assert!(records.iter().all(|record| from_notes(&task.notes, &record.id, &task.id).unwrap().is_some()));
            // Ordinary evidence is listed by task-relative path, but only a
            // routed build's manifest is strict: no new admission requirement.
            let manifest = task.notes.lines().find_map(|line| {
                serde_json::from_str::<EvidenceSync>(line.strip_prefix(EVIDENCE_SYNC)?).ok()
            }).expect("an ordinary park records its evidence manifest");
            assert!(!manifest.strict);
        }).await.unwrap();
    }

    /// cas-8018 (GH #2268): a routed park parks when an uncited top-level
    /// file's upload fails; the ledger and the files it cites are published,
    /// the failed file is listed as unpublished, and its upload is retried
    /// once. cas-fd1c (GH #2155, #2203): a nested source snapshot copied into
    /// the evidence directory is listed and never uploaded.
    #[tokio::test(flavor = "multi_thread")]
    async fn routed_park_publishes_cited_evidence_when_an_uncited_copy_fails_gh2268() {
        use wiremock::matchers::body_partial_json;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        // Delayed begin/PUT and a stalled complete reproduce an operation
        // chain that previously received a fresh timeout at every stage.
        for stalled in [true, false] {
            let server = MockServer::start().await;
            let ledger = "schema: 2\ncovers: cas-2268-fixture\n\n\
                | id | cell | expected | observed | verdict | label | evidence path | defect task |\n\
                | --- | --- | --- | --- | --- | --- | --- | --- |\n\
                | M01 | probe | ok [source: a.rs:1] | terminal: ok | PASS | fixture | green.log | - |\n";
            let evidence: [(&str, &[u8]); 4] = [
                ("LEDGER.md", ledger.as_bytes()), ("green.log", b"test: PASS\n"),
                ("brand-default-develop-light-desktop.png", b"\x89PNG old capture"),
                ("decided-before-source/apps/backend/.env.graphiti", b"NOT=evidence\n"),
            ];
            for (index, (name, _)) in evidence.iter().enumerate() {
                if name.contains('/') { continue; }
                let leaf = Path::new(name).file_name().unwrap().to_str().unwrap();
                let optional = leaf.ends_with(".png");
                if optional && !stalled {
                    Mock::given(method("POST")).and(path("/api/artifacts/begin"))
                        .and(body_partial_json(serde_json::json!({"name": leaf})))
                        .respond_with(ResponseTemplate::new(503)).expect(2).mount(&server).await;
                    continue;
                }
                let delay = if optional { std::time::Duration::from_secs(3) } else { std::time::Duration::ZERO };
                Mock::given(method("POST")).and(path("/api/artifacts/begin"))
                    .and(body_partial_json(serde_json::json!({"name": leaf})))
                    .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                        "artifact_id": format!("cloud-{index}"),
                        "upload_url": format!("{}/blob/{index}", server.uri()), "required_headers": {}}))
                        .set_delay(delay))
                    .mount(&server).await;
                Mock::given(method("PUT")).and(path(format!("/blob/{index}")))
                    .respond_with(ResponseTemplate::new(200).set_delay(delay)).mount(&server).await;
                Mock::given(method("POST")).and(path(format!("/api/artifacts/cloud-{index}/complete")))
                    .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"url": null}))
                        .set_delay(if optional { std::time::Duration::from_secs(30) } else { std::time::Duration::ZERO }))
                    .mount(&server).await;
            }
            let mut task = cas_types::Task::new("cas-2268-fixture".into(), "approved routed delivery".into());
            task.notes = format!("BUILD_ROUTE: {}", serde_json::json!({
                "key": "route-1", "source_machine_id": "prowl-fixture", "sender_agent_id": "sup",
                "recipient_agent_id": "worker", "recipient": "worker", "free_slots": 1, "load_per_cpu": null}));
            let mut parking = PeerReviewer::new(&server.uri(), task);
            env.set("CAS_ROOT", &parking.root);
            for (name, bytes) in &evidence {
                let file = parking.evidence_dir().join(name);
                std::fs::create_dir_all(file.parent().unwrap()).unwrap();
                std::fs::write(file, bytes).unwrap();
            }
            // GH #2203: a close that cites artifact ids and named evidence in
            // prose, beside a `--cas-root` command, reads no checkout file. The
            // citation scan admits only target/, the evidence directory and
            // artifacts paths, and skips directories.
            for name in ["Cargo.toml", "README.md", "Dockerfile", "schema.sql", ".prettierrc"] {
                std::fs::write(parking.repo.join(name), "checkout file\n").unwrap();
            }
            parking.task.deliverables.parked_close_reason = Some(format!(
                "PASS: art-0123abcd green.log Cargo.toml; cas factory worker-check --cas-root {}/.cas -- nextest run -p cas --lib",
                parking.repo.display()));
            tokio::task::spawn_blocking(move || {
                let started = std::time::Instant::now();
                publish_routed_evidence(&parking.root, &parking.task, &parking.repo).unwrap();
                let elapsed = started.elapsed();
                eprintln!("GH2268 stalled={stalled}: publication completed in {elapsed:?}");
                assert!(elapsed <= PUBLISH_BUDGET + std::time::Duration::from_secs(5), "publication took {elapsed:?}");
                let task = crate::store::open_task_store(&parking.root).unwrap().get(&parking.task.id).unwrap();
                let manifest = task.notes.lines().find_map(|line| {
                    serde_json::from_str::<EvidenceSync>(line.strip_prefix(EVIDENCE_SYNC)?).ok()
                }).expect("the park records its manifest");
                assert!(manifest.strict);
                let names: Vec<&str> = manifest.files.iter().map(|file| file.name.as_str()).collect();
                assert_eq!(names, ["LEDGER.md", "green.log"]);
                assert!(task.notes.contains("brand-default-develop-light-desktop.png"), "{}", task.notes);
                assert!(task.notes.contains("decided-before-source/apps/backend/.env.graphiti"), "{}", task.notes);
                for checkout in ["Cargo.toml", "Dockerfile", "schema.sql", ".prettierrc"] {
                    assert!(!task.notes.contains(checkout), "{checkout} swept: {}", task.notes);
                }
            }).await.unwrap();
            let requests = server.received_requests().await.unwrap();
            let begins: Vec<serde_json::Value> = requests.iter()
                .filter(|request| request.url.path() == "/api/artifacts/begin")
                .map(|request| serde_json::from_slice(&request.body).unwrap()).collect();
            assert_eq!(begins[0]["name"], "LEDGER.md");
            assert_eq!(begins[1]["name"], "green.log");
            assert_eq!(begins.len(), 4, "one retry for the optional file, no upload of the source copy");
            if stalled {
                assert_eq!(requests.iter().filter(|request| request.url.path() == "/api/artifacts/cloud-2/complete").count(), 1);
            }
        }
    }

    /// cas-1797 (GH #2267): a routed delivery whose evidence a peer already
    /// holds as committed, synced proof re-parks during a Cloud upload outage
    /// by citing that exact proof. A required file with no such proof still
    /// refuses the park.
    #[tokio::test(flavor = "multi_thread")]
    async fn routed_park_reuses_synced_peer_proof_during_an_upload_outage_gh2267() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let server = MockServer::start().await;
        Mock::given(method("POST")).and(path("/api/artifacts/begin"))
            .respond_with(ResponseTemplate::new(503)).mount(&server).await;
        let ledger = "schema: 2\ncovers: cas-2267-fixture\n\n\
            | id | cell | expected | observed | verdict | label | evidence path | defect task |\n\
            | --- | --- | --- | --- | --- | --- | --- | --- |\n\
            | M01 | home | ok [source: a.rs:1] | visible: ok | PASS | fixture | shots/home.png | - |\n";
        let evidence: [(&str, &[u8]); 2] = [("LEDGER.md", ledger.as_bytes()), ("shots/home.png", b"\x89PNG approved capture")];
        let mut task = cas_types::Task::new("cas-2267-fixture".into(), "approved routed delivery".into());
        task.notes = format!("BUILD_ROUTE: {}", serde_json::json!({
            "key": "route-1", "source_machine_id": "prowl-fixture", "sender_agent_id": "sup",
            "recipient_agent_id": "worker", "recipient": "worker", "free_slots": 1, "load_per_cpu": null}));
        // The proof the peer's earlier park published and its reviewer fetched.
        for (index, (name, bytes)) in evidence.iter().enumerate() {
            let record = PublishedArtifact {
                id: format!("art-peer{index}"), task_id: "cas-2267-fixture".into(),
                name: Path::new(name).file_name().unwrap().to_str().unwrap().into(),
                mime: "application/octet-stream".into(), size_bytes: bytes.len() as u64,
                sha256: format!("{:x}", <sha2::Sha256 as sha2::Digest>::digest(bytes)),
                cloud_artifact_id: Some(format!("cloud-peer{index}")), status: "committed".into(),
                cloud_url: None, slack_permalink: None, created_at: "2026-11-21T08:00:00Z".into(),
                revocation: None,
            };
            task.notes.push_str(&format!("\n{MARKER}{}", serde_json::to_string(&record).unwrap()));
        }
        let mut parking = PeerReviewer::new(&server.uri(), task);
        env.set("CAS_ROOT", &parking.root);
        for (name, bytes) in &evidence {
            let file = parking.evidence_dir().join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, bytes).unwrap();
        }
        let unreviewed = parking.evidence_dir().join("shots/new.png");
        std::fs::write(&unreviewed, b"\x89PNG never published").unwrap();
        parking.task.deliverables.parked_close_reason = Some(format!("VERIFIED `{}`", unreviewed.display()));
        tokio::task::spawn_blocking(move || {
            let refused = publish_routed_evidence(&parking.root, &parking.task, &parking.repo).unwrap_err();
            assert!(refused.contains("new.png") && !refused.contains("home.png") && !refused.contains("LEDGER.md"), "{refused}");
            std::fs::remove_file(&unreviewed).unwrap();
            parking.task.deliverables.parked_close_reason = None;
            publish_routed_evidence(&parking.root, &parking.task, &parking.repo).unwrap();
            let task = crate::store::open_task_store(&parking.root).unwrap().get(&parking.task.id).unwrap();
            let manifest = task.notes.lines().find_map(|line| {
                serde_json::from_str::<EvidenceSync>(line.strip_prefix(EVIDENCE_SYNC)?).ok()
            }).expect("the park records its manifest");
            let files: Vec<(&str, Option<&str>)> = manifest.files.iter()
                .map(|file| (file.name.as_str(), file.artifact_id.as_deref())).collect();
            assert_eq!(files, [("LEDGER.md", Some("art-peer0")), ("shots/home.png", Some("art-peer1"))]);
        }).await.unwrap();
        // Only the unreviewed capture was attempted (once, plus one retry).
        let begins = server.received_requests().await.unwrap().iter()
            .filter(|request| request.url.path() == "/api/artifacts/begin").count();
        assert_eq!(begins, 2);
    }

    /// GH #2079: a delivery parked on one machine leaves nested captures
    /// (`proof/`) and top-level logs in its task evidence directory. The merge
    /// owner, holding only the synced task notes, restores each at the path the
    /// ledger cites; an artifact it cannot fetch never blocks the dispatch.
    /// cas-fd1c (GH #2212): a ledger citing nested captures the way the ledger
    /// gate accepts (one path per cell: relative, absolute inside the
    /// directory, or another machine's artifacts path) publishes and restores
    /// them; an uncited terminal-qa receipt still travels and uncited raw
    /// captures stay unpublished. Bundle-receipt reuse on a no-code close is
    /// covered through the real close handler in `no_code_close_tests`.
    #[tokio::test(flavor = "multi_thread")]
    async fn ordinary_park_evidence_reaches_the_merge_owner_at_cited_paths_gh2079() {
        use wiremock::matchers::body_partial_json;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let server = MockServer::start().await;
        let task = cas_types::Task::new("cas-2079-fixture".into(), "parked elsewhere".into());
        let parking = PeerReviewer::new(&server.uri(), task);
        let absolute = parking.evidence_dir().join("proof/shot.png");
        let foreign = "/Users/other/.cas/artifacts/cassy-0123456789ab/cas-2079-fixture/proof/second.png";
        let row = |id: &str, cited: &str| format!(
            "| {id} | home | ok [source: a.rs:1] | visible: ok | PASS | fixture | {cited} | - |\n");
        let ledger = format!("schema: 2\ncovers: cas-2079-fixture\n\
            cells=3; PASS=3; FAIL=0; NOT EXERCISED=0; NOT A ROUTE=0\nlabels: fixture=3\n\n\
            | id | cell | expected | observed | verdict | label | evidence path | defect task |\n\
            | --- | --- | --- | --- | --- | --- | --- | --- |\n{}{}{}",
            row("M01", "web-green.log"), row("M02", &absolute.display().to_string()), row("M03", foreign));
        let evidence: [(&str, &[u8]); 5] = [("proof/shot.png", b"\x89PNG capture"),
            ("proof/second.png", b"\x89PNG second"), ("web-green.log", b"web: PASS\n"),
            ("LEDGER.md", ledger.as_bytes()), ("terminal-qa/status/report.md", b"terminal-qa: PASS\n")];
        let raw = "raw/requests/0001.json";
        for (index, (name, bytes)) in evidence.iter().enumerate() {
            let leaf = Path::new(name).file_name().unwrap().to_str().unwrap();
            Mock::given(method("POST")).and(path("/api/artifacts/begin"))
                .and(body_partial_json(serde_json::json!({"name": leaf})))
                .respond_with(ResponseTemplate::new(201).set_body_json(serde_json::json!({
                    "artifact_id": format!("cloud-{index}"),
                    "upload_url": format!("{}/blob/{index}", server.uri()), "required_headers": {}})))
                .mount(&server).await;
            Mock::given(method("PUT")).and(path(format!("/blob/{index}")))
                .respond_with(ResponseTemplate::new(200)).mount(&server).await;
            Mock::given(method("POST")).and(path(format!("/api/artifacts/cloud-{index}/complete")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"url": null})))
                .mount(&server).await;
            Mock::given(method("GET")).and(path(format!("/api/artifacts/cloud-{index}/url")))
                .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                    "artifact_id": format!("cloud-{index}"),
                    "url": format!("{}/blob/{index}", server.uri()), "size_bytes": bytes.len()})))
                .mount(&server).await;
            Mock::given(method("GET")).and(path(format!("/blob/{index}")))
                .respond_with(ResponseTemplate::new(200).set_body_bytes(bytes.to_vec()))
                .mount(&server).await;
        }
        env.set("CAS_ROOT", &parking.root);
        for (name, bytes) in evidence.iter().chain([&(raw, &b"{\"model\":\"raw\"}"[..])]) {
            let file = parking.evidence_dir().join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, bytes).unwrap();
        }
        // The fixture ledger is one the gate itself accepts.
        let checked = crate::qa_ledger::check(&parking.evidence_dir().join("LEDGER.md"), 0);
        assert!(checked.ok, "{:?}", checked.diagnostics);
        let (parked, parking_dir) = tokio::task::spawn_blocking(move || {
            publish_routed_evidence(&parking.root, &parking.task, &parking.repo).unwrap();
            let parked = crate::store::open_task_store(&parking.root).unwrap().get(&parking.task.id).unwrap();
            (parked, parking.evidence_dir())
        }).await.unwrap();
        std::fs::remove_dir_all(&parking_dir).unwrap();

        let owner = PeerReviewer::new(&server.uri(), parked);
        let owner_dir = owner.evidence_dir();
        let (owner_task, owner_root) = (owner.task.clone(), owner.root.clone());
        env.set("CAS_ROOT", &owner_root);
        tokio::task::spawn_blocking(move || {
            materialize_peer_evidence(&owner_root, &owner_task).unwrap();
        }).await.unwrap();
        for (name, bytes) in &evidence {
            assert_eq!(&std::fs::read(owner_dir.join(name)).unwrap(), bytes, "{name}");
        }
        assert!(!owner_dir.join(raw).exists());
        let begins = server.received_requests().await.unwrap().iter()
            .filter(|request| request.url.path() == "/api/artifacts/begin").count();
        assert_eq!(begins, evidence.len(), "only declared evidence is uploaded");
    }

}
