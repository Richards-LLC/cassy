//! Verified before/after evidence, selected by task or commit provenance.
use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use cas_store::{HistoryStore, LinkConfidence};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{Finding, Receipt, UsageError};
use crate::config::{Config, factory_task_artifact_dirs, resolved_factory_artifacts_root};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SideCapture {
    pub png: String,
    pub jpg: String,
    pub sha256: String,
    pub jpg_sha256: String,
    #[serde(rename = "ref")]
    pub reference: String,
    pub source: String,
    pub captured_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Pair {
    pub id: String,
    pub surface: String,
    pub viewport: [u32; 2],
    pub scheme: String,
    #[serde(default)]
    pub masked: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub before: Option<SideCapture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub after: Option<SideCapture>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PairManifest {
    pub schema: u32,
    pub task_id: String,
    pub base_sha: String,
    pub head_sha: String,
    pub origin: String,
    #[serde(default)]
    pub pairs: Vec<Pair>,
    #[serde(default)]
    pub unpaired: Vec<Pair>,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

#[derive(Debug, Clone)]
pub enum Selection {
    Task(String),
    Range(String),
}

#[derive(Debug, Clone)]
pub struct PairsRequest {
    pub root: PathBuf,
    pub cas_root: PathBuf,
    pub selection: Selection,
    /// Explicit input directory for a task; otherwise its durable qa/pairs.
    pub dir: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SelectedPair {
    pub task_id: String,
    pub directory: String,
    #[serde(flatten)]
    pub pair: Pair,
}

#[derive(Debug, Serialize)]
pub struct PairsReceipt {
    pub schema: &'static str,
    pub pairs: Vec<SelectedPair>,
    pub unpaired: Vec<SelectedPair>,
    pub manifests: Vec<String>,
    pub unlinked: Vec<String>,
    pub warnings: Vec<String>,
    pub findings: Vec<Finding>,
}

impl Receipt for PairsReceipt {
    fn findings(&self) -> &[Finding] {
        &self.findings
    }
    fn warnings(&self) -> &[String] {
        &self.warnings
    }
}

pub fn valid_id(id: &str) -> bool {
    id.as_bytes().first().is_some_and(u8::is_ascii_alphanumeric)
        && id.len() <= 120
        && id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
}

pub fn task_dir(config: &Config, cas_root: &Path, task: &str) -> PathBuf {
    let base = resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    factory_task_artifact_dirs(cas_root, &base, task)[0].join("qa/pairs")
}

pub fn pairs(request: &PairsRequest) -> anyhow::Result<PairsReceipt> {
    let mut receipt = PairsReceipt {
        schema: "cas.deliverable.pairs/1",
        pairs: vec![],
        unpaired: vec![],
        manifests: vec![],
        unlinked: vec![],
        warnings: vec![],
        findings: vec![],
    };
    let config = Config::load(&request.cas_root)?;
    let tasks = match &request.selection {
        Selection::Task(task) => {
            if !valid_id(task) {
                return Err(UsageError("--task requires a task identifier".into()).into());
            }
            vec![task.clone()]
        }
        Selection::Range(range) => {
            if request.dir.is_some() {
                return Err(UsageError("--dir requires --task, not --range".into()).into());
            }
            let (from, to) = range
                .split_once("..")
                .filter(|(a, b)| !a.is_empty() && !b.is_empty() && !b.starts_with('.'))
                .ok_or_else(|| UsageError("--range requires BASE..HEAD".into()))?;
            let from = revision(&request.root, from)?;
            let to = revision(&request.root, to)?;
            let output = super::asset::run(
                Command::new("git")
                    .current_dir(&request.root)
                    .args(["rev-list", &format!("{from}..{to}")]),
                Duration::from_secs(10),
                "git",
            )?;
            if !output.status.success() {
                return Err(UsageError("cannot enumerate --range commits".into()).into());
            }
            let shas = String::from_utf8(output.stdout)?
                .lines()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            let store = cas_store::SqliteHistoryStore::open(&request.cas_root)?;
            let provenance =
                store.resolve_provenance(&crate::history::repository_id(&request.root), &shas)?;
            let mut tasks = BTreeSet::new();
            for sha in shas {
                let mut linked = false;
                if let Some(commit) = provenance.get(&sha) {
                    for link in &commit.links {
                        if !link.ambiguous && link.confidence >= LinkConfidence::Medium {
                            if let Some(task) =
                                link.task_id.as_deref().filter(|task| valid_id(task))
                            {
                                tasks.insert(task.to_owned());
                                linked = true;
                            }
                        }
                    }
                }
                if !linked {
                    receipt.unlinked.push(sha);
                }
            }
            if !receipt.unlinked.is_empty() {
                receipt.warnings.push(format!(
                    "{} commit(s) have no unambiguous task provenance; run cas history index",
                    receipt.unlinked.len()
                ));
            }
            tasks.into_iter().collect()
        }
    };
    for task in tasks {
        let dir = request.dir.clone().unwrap_or_else(|| {
            let primary = task_dir(&config, &request.cas_root, &task);
            if primary.join("pairs.json").exists() {
                return primary;
            }
            let base = resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
            let legacy =
                factory_task_artifact_dirs(&request.cas_root, &base, &task)[1].join("qa/pairs");
            if legacy.join("pairs.json").exists() {
                legacy
            } else {
                primary
            }
        });
        let path = dir.join("pairs.json");
        let manifest = match read_manifest(&path) {
            Ok(manifest) if manifest.schema == 1 && manifest.task_id == task => manifest,
            Ok(_) => {
                receipt.findings.push(finding(
                    "pairs-identity",
                    "pairs.json schema/task identity does not match the request",
                ));
                continue;
            }
            Err(error) => {
                receipt.findings.push(finding(
                    "pairs-unavailable",
                    format!("{task}: {error}; run cas deliverable capture --task {task}"),
                ));
                continue;
            }
        };
        receipt.manifests.push(path.display().to_string());
        receipt.findings.extend(manifest.findings);
        let mut seen = HashSet::new();
        for mut pair in manifest.pairs.into_iter().chain(manifest.unpaired) {
            let mut problem = None;
            if !valid_id(&pair.id)
                || !seen.insert(pair.id.clone())
                || pair.viewport.contains(&0)
                || !matches!(pair.scheme.as_str(), "light" | "dark")
            {
                problem = Some(finding(
                    "capture-identity",
                    "invalid or duplicate pair identity/framing",
                ));
            }
            if problem.is_none() {
                for side in [&pair.before, &pair.after].into_iter().flatten() {
                    if let Err(error) = verify_side(&dir, side) {
                        problem = Some(error);
                        break;
                    }
                }
            }
            if pair.before.is_none() || pair.after.is_none() {
                problem.get_or_insert_with(|| {
                    finding(
                        "unpaired-capture",
                        "one capture side is missing; incomplete evidence is excluded",
                    )
                });
            }
            if let Some(error) = problem {
                pair.reason = Some(error.message.clone());
                receipt.findings.push(finding(
                    &error.code,
                    format!("{task}/{}: {}", pair.id, error.message),
                ));
                receipt.unpaired.push(SelectedPair {
                    task_id: task.clone(),
                    directory: dir.display().to_string(),
                    pair,
                });
            } else {
                receipt.pairs.push(SelectedPair {
                    task_id: task.clone(),
                    directory: dir.display().to_string(),
                    pair,
                });
            }
        }
    }
    Ok(receipt)
}

pub(crate) fn finding(code: &str, message: impl Into<String>) -> Finding {
    Finding {
        code: code.into(),
        message: message.into(),
    }
}

pub(crate) fn read_manifest(path: &Path) -> anyhow::Result<PairManifest> {
    let mut file = fs::File::open(path)?;
    if file.metadata()?.len() > 8 * 1024 * 1024 {
        anyhow::bail!("pairs.json exceeds 8 MB");
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub(crate) fn write_manifest(dir: &Path, manifest: &PairManifest) -> anyhow::Result<()> {
    use std::io::Write;
    fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer_pretty(&mut file, manifest)?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(dir.join("pairs.json"))?;
    Ok(())
}

pub(crate) fn digest(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut bytes = [0_u8; 16384];
    loop {
        let count = file.read(&mut bytes)?;
        if count == 0 {
            break;
        }
        hash.update(&bytes[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn verify_side(dir: &Path, side: &SideCapture) -> Result<(), Finding> {
    if side.reference.is_empty()
        || side.source.is_empty()
        || chrono::DateTime::parse_from_rfc3339(&side.captured_at).is_err()
    {
        return Err(finding(
            "capture-provenance",
            "capture reference/source/time is missing or invalid",
        ));
    }
    for (relative, expected) in [(&side.png, &side.sha256), (&side.jpg, &side.jpg_sha256)] {
        let file = Path::new(relative);
        if file.is_absolute()
            || file
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return Err(finding(
                "capture-path",
                "capture path must remain inside its pairs directory",
            ));
        }
        let root = dir
            .canonicalize()
            .map_err(|_| finding("capture-path", "pairs directory is unavailable"))?;
        let path = dir
            .join(file)
            .canonicalize()
            .map_err(|_| finding("capture-path", "capture file is missing"))?;
        if !path.starts_with(root) || !path.is_file() {
            return Err(finding(
                "capture-path",
                "capture path escapes its pairs directory",
            ));
        }
        if expected.len() != 64
            || digest(&path).map_err(|_| finding("capture-digest", "capture is unreadable"))?
                != *expected
        {
            return Err(finding(
                "capture-digest",
                "capture bytes do not match their recorded digest",
            ));
        }
    }
    Ok(())
}

pub(crate) fn revision(root: &Path, value: &str) -> Result<String, UsageError> {
    let output = super::asset::run(
        Command::new("git").current_dir(root).args([
            "rev-parse",
            "--verify",
            "--end-of-options",
            &format!("{value}^{{commit}}"),
        ]),
        Duration::from_secs(10),
        "git",
    )?;
    if !output.status.success() {
        return Err(UsageError(
            "cannot resolve the requested git revision".into(),
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().into())
}
