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
    anyhow::ensure!(Path::new(&proof.source_path).is_absolute(), "QA source path must be absolute");
    anyhow::ensure!(proof.closure.name == "qa-evidence.tar.gz", "invalid QA closure name");
    Ok(())
}

fn relative(path: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(!path.as_os_str().is_empty() && path.components().all(|p| matches!(p, Component::Normal(_))),
        "QA evidence paths must be contained relative files");
    Ok(())
}

fn add_strings(value: &serde_json::Value, paths: &mut BTreeSet<PathBuf>) {
    match value {
        serde_json::Value::String(path) => { paths.insert(path.into()); }
        serde_json::Value::Array(values) => for value in values { add_strings(value, paths); },
        serde_json::Value::Object(values) => for value in values.values() { add_strings(value, paths); },
        _ => {}
    }
}

/// Build before any upload, and include only declared files (plus captures
/// named by pairs.json), never an arbitrary directory's credentials or logs.
pub(crate) fn prepare(context: &PublishContext<'_>, source: &Path)
    -> anyhow::Result<Option<(tempfile::TempDir, PathBuf)>>
{
    if source.file_name().is_none_or(|name| name != "bundle.json") { return Ok(None); }
    let Ok(manifest) = serde_json::from_reader::<_, serde_json::Value>(std::fs::File::open(source)?) else {
        return Ok(None);
    };
    if manifest["schema"] != 1 || !matches!(manifest["producer"].as_str(), Some("cas-qa-craft" | "journey")) {
        return Ok(None);
    }
    anyhow::ensure!(manifest["task_id"] == context.task_id, "QA manifest belongs to another task");
    let root = source.parent().ok_or_else(|| anyhow::anyhow!("QA manifest has no directory"))?;
    let mut paths = BTreeSet::new();
    add_strings(&manifest["files"], &mut paths);
    if let Some(pair_path) = manifest["files"]["pairs"].as_str() {
        let path = Path::new(pair_path);
        relative(path)?;
        let pair_file = checked_file(root, path)?;
        anyhow::ensure!(std::fs::metadata(&pair_file)?.len() <= ARTIFACT_MAX_BYTES, "QA pairs file exceeds artifact limit");
        let pairs: serde_json::Value = serde_json::from_reader(std::fs::File::open(pair_file)?)?;
        for phase in ["before", "after"] {
            for cell in pairs[phase].as_array().into_iter().flatten() {
                for key in ["png", "jpg"] {
                    if let Some(capture) = cell[key].as_str() {
                        relative(Path::new(capture))?;
                        paths.insert(path.parent().unwrap_or(Path::new("")).join(capture));
                    }
                }
            }
        }
    }
    paths.insert("bundle.json".into());
    anyhow::ensure!(paths.len() <= MAX_FILES, "QA evidence exceeds {MAX_FILES} files");
    std::fs::create_dir_all(&context.roots.task_artifacts_dir)?;
    let temp = tempfile::Builder::new().prefix("qa-export-").tempdir_in(&context.roots.task_artifacts_dir)?;
    let archive_path = temp.path().join("qa-evidence.tar.gz");
    let encoder = flate2::write::GzEncoder::new(std::fs::File::create(&archive_path)?, flate2::Compression::default());
    let mut archive = tar::Builder::new(encoder);
    let mut size = 0u64;
    for path in paths {
        let file = checked_file(root, &path)?;
        size = size.checked_add(std::fs::metadata(&file)?.len()).ok_or_else(|| anyhow::anyhow!("QA evidence too large"))?;
        anyhow::ensure!(size <= ARTIFACT_MAX_BYTES, "QA evidence closure exceeds the artifact byte limit");
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
        anyhow::ensure!(!std::fs::symlink_metadata(&file)?.file_type().is_symlink(), "QA evidence cannot contain symlinks");
    }
    anyhow::ensure!(file.is_file(), "QA evidence is not a regular file");
    Ok(file)
}

pub(crate) fn record(context: &PublishContext<'_>, manifest: &PublishOutcome, archive: &Path) -> anyhow::Result<()> {
    let closure = super::publish(context, archive)?;
    anyhow::ensure!(matches!(closure.disposition, super::PublishDisposition::Committed { .. }), "QA closure publication failed: {}", closure.disposition.summary());
    let proof = EvidenceProof {
        source_path: manifest.source.display().to_string(),
        manifest: manifest.artifact.clone(), closure: closure.artifact,
    };
    validate(&proof, &context.task_id)?;
    crate::store::open_task_store(&context.roots.project_root.join(".cas"))?
        .append_trusted_note(&context.task_id, &format!("{MARKER}{}", serde_json::to_string(&proof)?))?;
    Ok(())
}

/// Return a local producer citation only when the original host path is
/// absent. Existing files retain their original validation and diagnostics.
pub(crate) fn restore(cas_root: &Path, task: &Task, notes: &str, destination: &Path)
    -> anyhow::Result<Option<PathBuf>>
{
    let Some(cited) = crate::qa_evidence::cited_bundle_path(notes) else { return Ok(None); };
    let source = crate::qa_evidence::expand_home(&cited);
    if !std::fs::symlink_metadata(&source).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(None);
    }
    let proof = task.notes.lines().filter_map(parse).rev()
        .find(|proof| Path::new(&proof.source_path) == source);
    let Some(proof) = proof else { return Ok(None); };
    validate(&proof, &task.id)?;
    let (record, archive) = super::proof::fetch_task(cas_root, &task.id, &proof.closure.id)?;
    anyhow::ensure!(super::proof::same_artifact(&record, &proof.closure), "QA closure conflicts with published proof");
    std::fs::create_dir_all(destination)?;
    let temp = tempfile::Builder::new().prefix("qa-restored-").tempdir_in(destination)?;
    let decoder = flate2::read::GzDecoder::new(std::fs::File::open(archive)?);
    // Bound decompression too, including tar extension headers and padding.
    let mut archive = tar::Archive::new(decoder.take(ARTIFACT_MAX_BYTES + MAX_FILES as u64 * 1024 + 1024 * 1024));
    let mut paths = BTreeSet::new();
    let mut size = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        relative(&path)?;
        anyhow::ensure!(entry.header().entry_type() == tar::EntryType::Regular && paths.insert(path.clone()),
            "QA closure contains a link, special file or duplicate path");
        size = size.checked_add(entry.size()).ok_or_else(|| anyhow::anyhow!("QA evidence too large"))?;
        anyhow::ensure!(size <= ARTIFACT_MAX_BYTES && paths.len() <= MAX_FILES, "QA closure exceeds extraction limits");
        let file = temp.path().join(path);
        std::fs::create_dir_all(file.parent().unwrap())?;
        let mut output = std::fs::OpenOptions::new().write(true).create_new(true).open(file)?;
        let modified = std::time::UNIX_EPOCH.checked_add(std::time::Duration::from_secs(entry.header().mtime()?))
            .ok_or_else(|| anyhow::anyhow!("QA evidence timestamp is out of range"))?;
        let copied = std::io::copy(&mut entry, &mut output)?;
        anyhow::ensure!(copied == entry.size(), "QA closure file is truncated");
        output.flush()?;
        output.set_times(std::fs::FileTimes::new().set_modified(modified))?;
    }
    let manifest = temp.path().join("bundle.json");
    let (sha, size) = super::digest_file(&manifest)?;
    anyhow::ensure!(sha == proof.manifest.sha256 && size == proof.manifest.size_bytes, "QA closure has another manifest");
    Ok(Some(temp.keep().join("bundle.json")))
}
