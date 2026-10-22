//! The evidence pack (finding 15): the one thing promotion reads.
//! `<artifacts_root>/<project>/release/<sha>/pack.json` lists served
//! components, rows, the flag snapshot, PR coverage, fixture receipts,
//! resource use, suite timings and media `{path, sha256, bytes}`. Media are
//! local files under the pack's `media/` directory, never git.
use super::ServedComponent;
use super::coverage::PrCoverage;
use super::fixtures::FixtureReceipt;
use super::flags::FlagSnapshot;
use super::verdict::{Gate, RowVerdict, gate};
use crate::qa_readback::Readback;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const SCHEMA_VERSION: u32 = 1;
pub const PACK_FILE: &str = "pack.json";
pub const MEDIA_DIR: &str = "media";
/// Media retention removed, beside `pack.json`, whose bytes never change.
pub const PRUNED_FILE: &str = "pruned.json";

/// Why a pack cannot be sealed or read.
#[derive(Debug, thiserror::Error)]
pub enum PackError {
    #[error("pack location: {0}")]
    Location(String),
    #[error("pack media invalid: {0}")]
    Media(String),
    #[error("pack carries {0}; redact it, and keep secrets out of depends_on_flags")]
    Secret(&'static str),
    #[error("pack gate {stored:?} differs from the gate its rows derive ({derived:?})")]
    Gate { stored: super::verdict::GateVerdict, derived: super::verdict::GateVerdict },
    #[error("pack io: {0}")]
    Io(String),
}

/// Served components at gate start and, once sealed, at the end.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Served {
    pub start: Vec<ServedComponent>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end: Option<Vec<ServedComponent>>,
}

/// One row: a journey, or one golden-set cell × run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: String,
    pub journey: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cell: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<u32>,
    pub verdict: RowVerdict,
    /// Media paths (entries of [`Pack::media`]) captured by this row.
    #[serde(default)]
    pub frames: Vec<String>,
    #[serde(default)]
    pub readbacks: Vec<Readback>,
    /// Hand-walk minutes for a manual row.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manual_minutes: Option<u32>,
    /// PRs a manual row was recorded to cover.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covers_prs: Vec<u64>,
    /// Direct pushes (commit SHAs, at least 7 hex) a manual row covers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub covers_commits: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SuiteTiming {
    pub name: String,
    pub started_at: DateTime<Utc>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<DateTime<Utc>>,
}

/// A media file retention deleted: the digest it had, so the pack stays
/// loadable and a receipt bound to `pack.json` bytes stays valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pruned {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
    pub pruned_at: DateTime<Utc>,
}

/// The entries retention removed from the pack in `dir` (empty when none).
pub fn pruned(dir: &Path) -> Result<Vec<Pruned>, PackError> {
    match std::fs::read_to_string(dir.join(PRUNED_FILE)) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| PackError::Io(format!("{PRUNED_FILE}: {e}"))),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(PackError::Io(error.to_string())),
    }
}

/// Records `entries` as pruned in `dir` (atomically, before their deletion).
pub(super) fn record_pruned(dir: &Path, entries: &[Pruned]) -> Result<(), PackError> {
    let mut all = pruned(dir)?;
    all.extend(entries.iter().filter(|e| !all.iter().any(|p| p.path == e.path)).cloned().collect::<Vec<_>>());
    write_atomic(dir, PRUNED_FILE, &serde_json::to_vec_pretty(&all).map_err(|e| PackError::Io(e.to_string()))?)
}

/// Writes `dir/name` through a uniquely named temp file in `dir`, then an
/// atomic rename, so concurrent writers (prune, seal) never share a temp.
pub(super) fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), PackError> {
    use std::io::Write;
    let io = |e: std::io::Error| PackError::Io(e.to_string());
    let mut temp = tempfile::Builder::new().prefix(&format!(".{name}.")).tempfile_in(dir).map_err(io)?;
    temp.write_all(bytes).map_err(io)?;
    temp.persist(dir.join(name)).map_err(|e| io(e.error))?;
    Ok(())
}

/// One local media file, relative to the pack directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Media {
    pub path: String,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pack {
    pub schema_version: u32,
    pub project: String,
    pub sha: String,
    pub created_at: DateTime<Utc>,
    pub served: Served,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub flags: Option<FlagSnapshot>,
    pub rows: Vec<Row>,
    #[serde(default)]
    pub coverage: Vec<PrCoverage>,
    #[serde(default)]
    pub fixtures: Vec<FixtureReceipt>,
    /// What resource preflight measured and granted (C4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resources: Option<super::resources::ResourcePlan>,
    #[serde(default)]
    pub suites: Vec<SuiteTiming>,
    #[serde(default)]
    pub media: Vec<Media>,
    /// Hand-walk debt this gate measured (finding 21). Never affects the gate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hand_walk: Option<HandWalk>,
    /// Derived from rows and coverage at seal; a reader recomputes it.
    pub gate: Gate,
}

/// Manual minutes one journey cost this gate, with its automation owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandWalked {
    pub journey: String,
    pub minutes: u32,
    pub rows: u32,
    /// The registry's `automation`; `None` is unowned debt.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub automation: Option<crate::qa_registry::Automation>,
}

/// Hand-walk debt measured by one gate: manual rows (those with
/// `manual_minutes`), their total, and each journey, most minutes first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HandWalk {
    pub total_minutes: u32,
    pub rows: u32,
    pub journeys: Vec<HandWalked>,
}

impl HandWalk {
    /// Totals the manual rows and names each journey's automation owner.
    pub fn measure(rows: &[Row], registry: Option<&crate::qa_registry::Registry>) -> Self {
        let mut by_journey: std::collections::BTreeMap<&str, (u32, u32)> = Default::default();
        for row in rows {
            if let Some(minutes) = row.manual_minutes {
                let entry = by_journey.entry(&row.journey).or_default();
                entry.0 += minutes;
                entry.1 += 1;
            }
        }
        let mut journeys: Vec<HandWalked> = by_journey
            .into_iter()
            .map(|(journey, (minutes, rows))| HandWalked {
                journey: journey.into(),
                minutes,
                rows,
                automation: registry
                    .and_then(|r| r.journeys.iter().find(|j| j.id == journey))
                    .and_then(|j| j.automation.clone())
                    .filter(|a| !a.owner.trim().is_empty()),
            })
            .collect();
        journeys.sort_by(|a, b| b.minutes.cmp(&a.minutes).then_with(|| a.journey.cmp(&b.journey)));
        Self {
            total_minutes: journeys.iter().map(|j| j.minutes).sum(),
            rows: journeys.iter().map(|j| j.rows).sum(),
            journeys,
        }
    }

    /// `N manual minutes over R rows; top: `j` M min (owner …), …` for the
    /// `top` costliest journeys; `None` when nothing was hand-walked.
    pub fn summary(&self, top: usize) -> Option<String> {
        if self.rows == 0 {
            return None;
        }
        let offenders: Vec<String> = self
            .journeys
            .iter()
            .take(top)
            .map(|j| {
                let owner = j.automation.as_ref().map_or("unowned".into(), |a| a.summary());
                format!("`{}` {} min ({})", super::printable(&j.journey), j.minutes, super::printable(&owner))
            })
            .collect();
        Some(format!(
            "{} manual minutes over {} rows; top: {}",
            self.total_minutes,
            self.rows,
            offenders.join(", ")
        ))
    }
}

/// `<artifacts_root>/<project>/release/<sha>` for the project whose Cassy
/// store is `cas_root`; the SHA must be a full 40-hex commit.
pub fn pack_dir(cas_root: &Path, sha: &str) -> Result<PathBuf, PackError> {
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(PackError::Location("sha must be a full 40-hex commit".into()));
    }
    let config = crate::config::Config::load(cas_root).map_err(|e| PackError::Location(e.to_string()))?;
    let paths = crate::config::resolved_factory_artifact_paths(cas_root, config.factory().artifacts_root.as_deref());
    Ok(paths.project_root.join("release").join(sha.to_ascii_lowercase()))
}

impl Pack {
    pub fn new(project: &str, sha: &str, served: Vec<ServedComponent>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            project: project.into(),
            sha: sha.to_ascii_lowercase(),
            created_at: Utc::now(),
            served: Served { start: served, end: None },
            flags: None,
            rows: Vec::new(),
            coverage: Vec::new(),
            fixtures: Vec::new(),
            resources: None,
            suites: Vec::new(),
            media: Vec::new(),
            hand_walk: None,
            gate: gate([], Vec::new()),
        }
    }

    /// The gate the rows and PR coverage derive.
    pub fn derive_gate(&self) -> Gate {
        gate(
            self.rows.iter().map(|row| (row.id.as_str(), &row.verdict)),
            self.coverage.iter().filter_map(PrCoverage::owed).collect(),
        )
    }

    /// Records a file already under `dir/media/` with its digest.
    pub fn add_media(&mut self, dir: &Path, relative: &str) -> Result<&Media, PackError> {
        let (sha256, bytes) = digest(&media_path(dir, relative)?)?;
        self.media.retain(|m| m.path != relative);
        self.media.push(Media { path: relative.into(), sha256, bytes });
        Ok(self.media.last().expect("just pushed"))
    }

    /// Verifies media, derives the gate, scans for secrets (the flag snapshot
    /// included) and writes `pack.json` atomically. Returns its path.
    ///
    /// A reseal (C6) verifies against the pruned records it will keep, but
    /// rewrites `pruned.json` only after the new `pack.json` is in place: a
    /// reseal that fails leaves the previous pack and its records loadable.
    pub fn seal(&mut self, dir: &Path) -> Result<PathBuf, PackError> {
        let records = pruned(dir)?;
        let keep = self.reconciled(dir, &records);
        self.verify_media_with(dir, &keep)?;
        self.gate = self.derive_gate();
        let text = serde_json::to_string_pretty(self).map_err(|e| PackError::Io(e.to_string()))?;
        if let Some(kind) = crate::qa_evidence::first_secret(&text) {
            return Err(PackError::Secret(kind));
        }
        std::fs::create_dir_all(dir).map_err(|e| PackError::Io(e.to_string()))?;
        write_atomic(dir, PACK_FILE, text.as_bytes())?;
        if keep.len() != records.len() {
            write_atomic(dir, PRUNED_FILE, &serde_json::to_vec_pretty(&keep).map_err(|e| PackError::Io(e.to_string()))?)?;
        }
        Ok(dir.join(PACK_FILE))
    }

    /// Reads a sealed pack; missing or changed media, or a stored gate that
    /// its rows no longer derive, make it invalid.
    pub fn load(dir: &Path) -> Result<Self, PackError> {
        let text = std::fs::read_to_string(dir.join(PACK_FILE)).map_err(|e| PackError::Io(e.to_string()))?;
        let pack: Self = serde_json::from_str(&text).map_err(|e| PackError::Io(format!("pack.json: {e}")))?;
        if pack.schema_version != SCHEMA_VERSION {
            return Err(PackError::Io(format!("unsupported pack schema {}", pack.schema_version)));
        }
        pack.verify_media(dir)?;
        let derived = pack.derive_gate();
        if derived != pack.gate {
            return Err(PackError::Gate { stored: pack.gate.verdict, derived: derived.verdict });
        }
        Ok(pack)
    }

    /// The pruned records a seal keeps: those whose entry this pack still
    /// lists with the same digest and whose file is still gone. Loading
    /// ignores the rest anyway; sealing drops them so the record stays small.
    fn reconciled(&self, dir: &Path, records: &[Pruned]) -> Vec<Pruned> {
        records
            .iter()
            .filter(|p| {
                std::fs::symlink_metadata(dir.join(&p.path)).is_err()
                    && self.media.iter().any(|m| m.path == p.path && m.sha256 == p.sha256 && m.bytes == p.bytes)
            })
            .cloned()
            .collect()
    }

    /// A present file must match its digest. A missing one is accepted only
    /// when retention recorded it pruned with that same digest.
    fn verify_media(&self, dir: &Path) -> Result<(), PackError> {
        self.verify_media_with(dir, &pruned(dir)?)
    }

    /// Records only ever excuse a missing file, and only with this pack's own
    /// path, sha256 and bytes. Records for present files (digest-checked
    /// anyway) or for paths the pack no longer lists are ignored, so a crash
    /// between a reseal's `pack.json` and `pruned.json` writes still loads.
    fn verify_media_with(&self, dir: &Path, pruned: &[Pruned]) -> Result<(), PackError> {
        for media in &self.media {
            let path = match media_path(dir, &media.path) {
                Ok(path) => path,
                Err(error) if std::fs::symlink_metadata(dir.join(&media.path)).is_err() => {
                    if pruned.iter().any(|p| p.path == media.path && p.sha256 == media.sha256 && p.bytes == media.bytes) {
                        continue;
                    }
                    return Err(error);
                }
                Err(error) => return Err(error),
            };
            let (sha256, bytes) = digest(&path)?;
            if sha256 != media.sha256 || bytes != media.bytes {
                return Err(PackError::Media(format!("{} changed since it was recorded", media.path)));
            }
        }
        for row in &self.rows {
            if let Some(frame) = row.frames.iter().find(|f| !self.media.iter().any(|m| &m.path == *f)) {
                return Err(PackError::Media(format!("row {} names unrecorded media {frame}", row.id)));
            }
        }
        Ok(())
    }
}

/// A media path must be a plain relative path under `media/`, and a regular
/// file rather than a link.
pub(super) fn media_path(dir: &Path, relative: &str) -> Result<PathBuf, PackError> {
    let path = Path::new(relative);
    let plain = path.components().all(|c| matches!(c, Component::Normal(_)));
    if !plain || path.components().next() != Some(Component::Normal(MEDIA_DIR.as_ref())) {
        return Err(PackError::Media(format!("{relative} is not under {MEDIA_DIR}/")));
    }
    // Every directory between the pack and the file must be real, too.
    let mut parent = dir.to_path_buf();
    for component in path.parent().into_iter().flat_map(Path::components) {
        parent.push(component);
        if !std::fs::symlink_metadata(&parent).is_ok_and(|meta| meta.file_type().is_dir()) {
            return Err(PackError::Media(format!("{relative} is not inside a real {MEDIA_DIR}/ directory")));
        }
    }
    let full = dir.join(path);
    match std::fs::symlink_metadata(&full) {
        Ok(meta) if meta.file_type().is_file() => Ok(full),
        Ok(_) => Err(PackError::Media(format!("{relative} is not a regular file"))),
        Err(_) => Err(PackError::Media(format!("{relative} is missing"))),
    }
}

pub(super) fn digest(path: &Path) -> Result<(String, u64), PackError> {
    let mut file = std::fs::File::open(path).map_err(|e| PackError::Io(e.to_string()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let read = file.read(&mut buffer).map_err(|e| PackError::Io(e.to_string()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        bytes += read as u64;
    }
    Ok((format!("{:x}", hasher.finalize()), bytes))
}
