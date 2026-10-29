//! A published producer manifest travels with the files it references. Restore
//! only into this host's task namespace; the ordinary QA validator still owns
//! freshness, role, head, trace, visual and journey validation.
use std::collections::BTreeSet;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use cas_store::PublishedArtifact;
use cas_types::{ARTIFACT_MAX_BYTES, Task};
use serde::{Deserialize, Serialize};

use super::{PublishContext, PublishOutcome};

const MARKER: &str = "QA_EVIDENCE_PROOF: ";
const MAX_FILES: usize = 2048;

#[derive(Deserialize, Serialize)]
struct EvidenceProof {
    source_path: String,
    manifest: PublishedArtifact,
    closure: PublishedArtifact,
    // Old archives placed the manifest at the archive root.
    #[serde(default = "default_manifest_path")]
    manifest_path: PathBuf,
}

fn default_manifest_path() -> PathBuf {
    "bundle.json".into()
}

pub(crate) fn valid_proof(line: &str, task: &str) -> bool {
    parse(line).is_some_and(|proof| validate(&proof, task).is_ok())
}

fn parse(line: &str) -> Option<EvidenceProof> {
    serde_json::from_str(line.strip_prefix(MARKER)?).ok()
}

fn validate(proof: &EvidenceProof, task: &str) -> anyhow::Result<()> {
    super::proof::validate(&proof.manifest, task)?;
    super::proof::validate(&proof.closure, task)?;
    relative(&proof.manifest_path)?;
    anyhow::ensure!(
        Path::new(&proof.source_path).is_absolute(),
        "QA source path must be absolute"
    );
    anyhow::ensure!(
        proof.closure.name == "qa-evidence.tar.gz",
        "invalid QA closure name"
    );
    Ok(())
}

fn relative(path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(
        !path.as_os_str().is_empty()
            && path.components().all(|p| matches!(p, Component::Normal(_))),
        "QA evidence paths must be contained relative files"
    );
    Ok(())
}

// Only declared path fields travel. files.parts also carries titles,
// labels and project names; treating arbitrary strings as paths breaks publish.
fn declared_files(value: &serde_json::Value) -> BTreeSet<PathBuf> {
    let mut paths = BTreeSet::new();
    for key in [
        "trace",
        "trace_actions",
        "receipt",
        "aria_yaml",
        "aria_json",
        "visual_qa",
        "visual_qa_json",
        "visual_qa_stdout",
        "visual_qa_baseline_json",
        "critique",
    ] {
        if let Some(path) = value[key].as_str().filter(|path| !path.is_empty()) {
            paths.insert(path.into());
        }
    }
    for key in ["cells", "a11y", "polish_screenshots"] {
        for path in value[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str())
        {
            paths.insert(path.into());
        }
    }
    for part in value["parts"].as_array().into_iter().flatten() {
        for key in [
            "receipt",
            "aria_yaml",
            "aria_json",
            "trace",
            "trace_actions",
        ] {
            if let Some(path) = part[key].as_str().filter(|path| !path.is_empty()) {
                paths.insert(path.into());
            }
        }
    }
    paths
}

// The validator allows the pairs manifest anywhere inside the task, while
// regular bundle files and pair captures remain inside their own directory.
fn contained_file(root: &Path, candidate: &Path) -> anyhow::Result<PathBuf> {
    let resolved = candidate.canonicalize()?;
    let path = resolved
        .strip_prefix(root)
        .map_err(|_| anyhow::anyhow!("QA evidence escapes its containing directory"))?;
    // Also reject symlinks in the original spelling, before canonicalization.
    let mut current = PathBuf::new();
    for part in candidate.components() {
        current.push(part.as_os_str());
        anyhow::ensure!(
            !std::fs::symlink_metadata(&current)?
                .file_type()
                .is_symlink(),
            "QA evidence cannot contain symlinks"
        );
    }
    checked_file(root, path)
}

fn archive_root(context: &PublishContext<'_>, source: &Path) -> anyhow::Result<PathBuf> {
    for root in [
        &context.roots.task_artifacts_dir,
        &context.roots.legacy_task_artifacts_dir,
    ] {
        if let Ok(root) = root.canonicalize() {
            if source.starts_with(&root) {
                return Ok(root);
            }
        }
    }
    source
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| anyhow::anyhow!("QA manifest has no directory"))
}

/// Build before any upload, and include only declared files (plus captures
/// named by pairs.json), never an arbitrary directory's credentials or logs.
pub(crate) fn prepare(
    context: &PublishContext<'_>,
    source: &Path,
) -> anyhow::Result<Option<(tempfile::TempDir, PathBuf)>> {
    if source.file_name().is_none_or(|name| name != "bundle.json") {
        return Ok(None);
    }
    let Ok(manifest) =
        serde_json::from_reader::<_, serde_json::Value>(std::fs::File::open(source)?)
    else {
        return Ok(None);
    };
    if manifest["schema"] != 1
        || !matches!(
            manifest["producer"].as_str(),
            Some("cas-qa-craft" | "journey")
        )
    {
        return Ok(None);
    }
    anyhow::ensure!(
        manifest["task_id"] == context.task_id,
        "QA manifest belongs to another task"
    );
    let root = source
        .parent()
        .ok_or_else(|| anyhow::anyhow!("QA manifest has no directory"))?;
    let task_root = archive_root(context, source)?;
    let mut paths = BTreeSet::new();
    for path in declared_files(&manifest["files"]) {
        let file = contained_file(root, &root.join(path))?;
        paths.insert(file.strip_prefix(&task_root)?.to_path_buf());
    }
    if let Some(path) = manifest["deployed"]["deployment_proof"].as_str() {
        let file = contained_file(root, &root.join(path))?;
        paths.insert(file.strip_prefix(&task_root)?.to_path_buf());
    }
    if let Some(pair_path) = manifest["files"]["pairs"].as_str() {
        let pair_file = contained_file(&task_root, &root.join(pair_path))?;
        anyhow::ensure!(
            std::fs::metadata(&pair_file)?.len() <= ARTIFACT_MAX_BYTES,
            "QA pairs file exceeds artifact limit"
        );
        let pairs: serde_json::Value = serde_json::from_reader(std::fs::File::open(&pair_file)?)?;
        let pair_dir = pair_file
            .parent()
            .ok_or_else(|| anyhow::anyhow!("QA pairs has no directory"))?;
        for cell in ["pairs", "unpaired"]
            .into_iter()
            .flat_map(|key| pairs[key].as_array().into_iter().flatten())
        {
            for phase in ["before", "after"] {
                for key in ["png", "jpg"] {
                    if let Some(capture) = cell[phase][key].as_str() {
                        let file = contained_file(pair_dir, &pair_dir.join(capture))?;
                        paths.insert(file.strip_prefix(&task_root)?.to_path_buf());
                    }
                }
            }
        }
        paths.insert(pair_file.strip_prefix(&task_root)?.to_path_buf());
    }
    paths.insert(source.strip_prefix(&task_root)?.to_path_buf());
    anyhow::ensure!(
        paths.len() <= MAX_FILES,
        "QA evidence exceeds {MAX_FILES} files"
    );
    std::fs::create_dir_all(&context.roots.task_artifacts_dir)?;
    let temp = tempfile::Builder::new()
        .prefix("qa-export-")
        .tempdir_in(&context.roots.task_artifacts_dir)?;
    let archive_path = temp.path().join("qa-evidence.tar.gz");
    let encoder = flate2::write::GzEncoder::new(
        std::fs::File::create(&archive_path)?,
        flate2::Compression::default(),
    );
    let mut archive = tar::Builder::new(encoder);
    let mut size = 0u64;
    for path in paths {
        let file = checked_file(&task_root, &path)?;
        size = size
            .checked_add(std::fs::metadata(&file)?.len())
            .ok_or_else(|| anyhow::anyhow!("QA evidence too large"))?;
        anyhow::ensure!(
            size <= ARTIFACT_MAX_BYTES,
            "QA evidence closure exceeds the artifact byte limit"
        );
        archive.append_path_with_name(file, path)?;
    }
    archive.into_inner()?.finish()?;
    Ok(Some((temp, archive_path)))
}

fn checked_file(root: &Path, path: &Path) -> anyhow::Result<PathBuf> {
    relative(path)?;
    let mut file = root.to_path_buf();
    for part in path.components() {
        file.push(part.as_os_str());
        anyhow::ensure!(
            !std::fs::symlink_metadata(&file)?.file_type().is_symlink(),
            "QA evidence cannot contain symlinks"
        );
    }
    anyhow::ensure!(file.is_file(), "QA evidence is not a regular file");
    Ok(file)
}

pub(crate) fn record(
    context: &PublishContext<'_>,
    manifest: &PublishOutcome,
    archive: &Path,
) -> anyhow::Result<()> {
    let closure = super::publish(context, archive)?;
    anyhow::ensure!(
        matches!(
            closure.disposition,
            super::PublishDisposition::Committed { .. }
        ),
        "QA closure publication failed: {}",
        closure.disposition.summary()
    );
    let proof = EvidenceProof {
        source_path: manifest.source.display().to_string(),
        manifest: manifest.artifact.clone(),
        closure: closure.artifact,
        manifest_path: manifest
            .source
            .strip_prefix(archive_root(context, &manifest.source)?)?
            .into(),
    };
    validate(&proof, &context.task_id)?;
    crate::store::open_task_store(&context.roots.project_root.join(".cas"))?.append_trusted_note(
        &context.task_id,
        &format!("{MARKER}{}", serde_json::to_string(&proof)?),
    )?;
    Ok(())
}

/// Return a local producer citation only when the original host path is
/// absent. Existing files retain their original validation and diagnostics.
pub(crate) fn restore(
    cas_root: &Path,
    task: &Task,
    notes: &str,
    destination: &Path,
) -> anyhow::Result<Option<PathBuf>> {
    let Some(cited) = crate::qa_evidence::cited_bundle_path(notes) else {
        return Ok(None);
    };
    let source = crate::qa_evidence::expand_home(&cited);
    if !std::fs::symlink_metadata(&source).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound)
    {
        return Ok(None);
    }
    let proof = task
        .notes
        .lines()
        .filter_map(parse)
        .rev()
        .find(|proof| Path::new(&proof.source_path) == source);
    let Some(proof) = proof else {
        return Ok(None);
    };
    validate(&proof, &task.id)?;
    let (record, archive) = super::proof::fetch_task(cas_root, &task.id, &proof.closure.id)?;
    anyhow::ensure!(
        super::proof::same_artifact(&record, &proof.closure),
        "QA closure conflicts with published proof"
    );
    std::fs::create_dir_all(destination)?;
    let temp = tempfile::Builder::new()
        .prefix("qa-restored-")
        .tempdir_in(destination)?;
    let decoder = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
    // Bound decompression too, including tar extension headers and padding.
    let mut archive =
        tar::Archive::new(decoder.take(ARTIFACT_MAX_BYTES + MAX_FILES as u64 * 1024 + 1024 * 1024));
    let mut paths = BTreeSet::new();
    let mut size = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        relative(&path)?;
        anyhow::ensure!(
            entry.header().entry_type() == tar::EntryType::Regular && paths.insert(path.clone()),
            "QA closure contains a link, special file or duplicate path"
        );
        size = size
            .checked_add(entry.size())
            .ok_or_else(|| anyhow::anyhow!("QA evidence too large"))?;
        anyhow::ensure!(
            size <= ARTIFACT_MAX_BYTES && paths.len() <= MAX_FILES,
            "QA closure exceeds extraction limits"
        );
        let file = temp.path().join(path);
        let parent = file
            .parent()
            .ok_or_else(|| anyhow::anyhow!("QA closure file has no directory"))?;
        std::fs::create_dir_all(parent)?;
        let mut output = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(file)?;
        let modified = std::time::UNIX_EPOCH
            .checked_add(std::time::Duration::from_secs(entry.header().mtime()?))
            .ok_or_else(|| anyhow::anyhow!("QA evidence timestamp is out of range"))?;
        let copied = std::io::copy(&mut entry, &mut output)?;
        anyhow::ensure!(copied == entry.size(), "QA closure file is truncated");
        output.flush()?;
        output.set_times(std::fs::FileTimes::new().set_modified(modified))?;
    }
    let manifest = temp.path().join(&proof.manifest_path);
    let (sha, size) = super::digest_file(&manifest)?;
    anyhow::ensure!(
        sha == proof.manifest.sha256 && size == proof.manifest.size_bytes,
        "QA closure has another manifest"
    );
    // One retained directory per immutable closure. Verify cached bytes and
    // timestamps against this freshly verified archive before reusing it.
    let cached = destination.join(format!("qa-restored-{}", proof.closure.sha256));
    if cached.try_exists()? {
        anyhow::ensure!(
            std::fs::symlink_metadata(&cached)?.is_dir(),
            "QA restore cache is not a directory"
        );
        for path in paths {
            let old = checked_file(&cached, &path)?;
            let fresh = checked_file(temp.path(), &path)?;
            anyhow::ensure!(
                super::digest_file(&old)? == super::digest_file(&fresh)?
                    && std::fs::metadata(old)?.modified()?
                        == std::fs::metadata(fresh)?.modified()?,
                "QA restore cache differs from published evidence"
            );
        }
    } else {
        std::fs::rename(temp.path(), &cached)?;
    }
    Ok(Some(cached.join(&proof.manifest_path)))
}
