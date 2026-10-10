//! QA evidence at close (cas-0cd5): the implementer's cas-qa-craft bundle
//! must exist, be fresh, and prove a passing run before a user-facing
//! delivery can park or close. Also refuses deliveries that add Playwright /
//! JS skip markers (`test.fixme`, `.skip`, `.only`) without a stated reason.
//!
//! This module holds only validation. `close_ops` decides *when* it runs;
//! `qa_pass` decides *whether* a delivery is user-facing.
//!
//! Design: `docs/qa/evidence-close-gate.md`. Bundle contract: cas-c3b8 v1
//! (`cas-qa-craft` `references/evidence-bundle.md`).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

#[path = "qa_journeys.rs"]
pub mod journeys;

/// Note token that cites a bundle manifest: `qa-bundle: <abs>/bundle.json`.
pub const BUNDLE_CITATION: &str = "qa-bundle:";
/// Same-line (or preceding-line) escape for a deliberate skip marker.
pub const ALLOW_SKIP: &str = "cas-allow-skip:";
/// Where the contract's worked example and producing commands live.
pub const CONTRACT_REFERENCE: &str = "cas-qa-craft references/evidence-bundle.md";

const RUBRIC: [&str; 5] = [
    "distinctiveness",
    "fit",
    "hierarchy",
    "craft",
    "accessibility",
];
const FLOOR_DIMENSIONS: [&str; 3] = ["distinctiveness", "fit", "hierarchy"];
const FLOOR: u8 = 4;
const A11Y_MODES: [&str; 3] = ["forced-colors", "reduced-motion", "contrast-more"];
const POLISH_RENDERS: [&str; 4] = [
    "-light-desktop.png",
    "-light-phone.png",
    "-dark-desktop.png",
    "-dark-phone.png",
];

/// Everything the validator needs, resolved by the caller.
#[derive(Debug, Clone)]
pub struct EvidenceContext<'a> {
    pub task_id: &'a str,
    /// `<artifacts_root>/<project-key>/<task-id>`.
    pub task_artifacts_dir: &'a Path,
    /// Repository holding the delivered head (for committer time/ancestry).
    pub repo: &'a Path,
    /// Full SHA of the delivered commit.
    pub delivered_head: &'a str,
    /// The task's notes, where the `qa-bundle:` citation lives.
    pub notes: &'a str,
    /// cas-a6ab: `qa.deployed_origins`, the remote deployments whose
    /// authenticated runs may stand in for a local build when local auth is
    /// impossible. Empty means local builds only.
    pub deployed_origins: &'a [String],
}

/// Why evidence is refused. `problem` completes "its QA evidence bundle is …";
/// `command` is the exact next step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EvidenceRefusal {
    pub problem: String,
    pub command: String,
}

impl EvidenceRefusal {
    fn new(problem: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            problem: problem.into(),
            command: command.into(),
        }
    }
}

/// A bundle that passed every check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleReceipt {
    pub manifest: PathBuf,
    pub head_sha: String,
    pub passed_expects: usize,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    schema: u32,
    task_id: String,
    #[serde(default)]
    producer: String,
    head_sha: String,
    #[serde(default)]
    executed_head_sha: String,
    created_at: String,
    #[serde(default)]
    visual_change: bool,
    #[serde(default)]
    visual_qa_status: String,
    files: ManifestFiles,
    #[serde(default)]
    critique_score: std::collections::BTreeMap<String, i64>,
    /// cas-a6ab: present when the run was made against a deployed origin
    /// because local auth is impossible.
    #[serde(default)]
    deployed: Option<DeployedEvidence>,
}

/// cas-a6ab (GH #1023 finding 4): the provenance of a run against a deployed
/// origin instead of a local build. gabber-studio's staging backend rejects a
/// localhost origin on `/auth/session` (CORS), so an authenticated page has
/// no local real-build run. A deployed run counts only when it names the
/// reason, a configured origin, and proof that the deployment served the
/// delivered commit.
#[derive(Debug, Deserialize)]
struct DeployedEvidence {
    /// `scheme://host[:port]` of the deployment the run checked.
    #[serde(default)]
    origin: String,
    /// Why a local build could not be used (for example the backend's CORS
    /// rejecting a localhost origin on its session endpoint).
    #[serde(default)]
    reason: String,
    /// The commit the deployment serves.
    #[serde(default)]
    deployed_sha: String,
    /// Bundle-relative file recording what the deployment reported about
    /// itself (a version endpoint response, deployment metadata), naming
    /// `deployed_sha`.
    #[serde(default)]
    deployment_proof: String,
}

/// Shortest recorded reason accepted for a deployed-origin run.
const DEPLOYED_REASON_MIN_CHARS: usize = 20;

/// `scheme://host[:port]` of a URL, lower-cased, or `None` if it is not an
/// http(s) URL with a host.
pub fn url_origin(target: &str) -> Option<String> {
    let parsed = url::Url::parse(target.trim()).ok()?;
    if !matches!(parsed.scheme(), "http" | "https") {
        return None;
    }
    parsed.host_str()?;
    Some(parsed.origin().ascii_serialization().to_ascii_lowercase())
}

#[derive(Debug, Deserialize, Default)]
struct ManifestFiles {
    trace: Option<String>,
    trace_actions: Option<String>,
    receipt: Option<String>,
    aria_yaml: Option<String>,
    aria_json: Option<String>,
    #[serde(default)]
    cells: Vec<String>,
    #[serde(default)]
    a11y: Vec<String>,
    #[serde(default)]
    polish_screenshots: Vec<String>,
    visual_qa: Option<String>,
    visual_qa_json: Option<String>,
    visual_qa_stdout: Option<String>,
    /// cas-e371: with `visual_qa_status: "scoped"`, the strict run of the
    /// base build over the same pages, so findings already there are told
    /// apart from findings the delivery introduced.
    visual_qa_baseline_json: Option<String>,
    critique: Option<String>,
}

/// Newest `qa-bundle: <path>` citation in the notes that is the
/// implementer's own. cas-619f cites each independent round's bundle on the
/// same delivery task (`<task>/independent-qa/round-<n>/bundle.json`); those
/// are the reviewer's evidence and never stand in for, or shadow, the
/// implementer's bundle.
pub fn cited_bundle_path(notes: &str) -> Option<String> {
    notes
        .match_indices(BUNDLE_CITATION)
        .filter_map(|(index, token)| {
            notes[index + token.len()..]
                .split_whitespace()
                .next()
                .map(|raw| {
                    // Sentence punctuation may sit outside or inside quoting:
                    // "`/p/bundle.json`." and "(/p/bundle.json)." both cite /p/bundle.json.
                    raw.trim_start_matches(|ch: char| "(),[]{}<>\"'`;*".contains(ch))
                        .trim_end_matches(|ch: char| "(),[]{}<>\"'`;*.".contains(ch))
                        .to_string()
                })
                .filter(|path| !path.is_empty() && !path.contains("/independent-qa/"))
        })
        .last()
}

pub(crate) fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir()
            .map(|home| home.join(rest))
            .unwrap_or_else(|| PathBuf::from(path)),
        None => PathBuf::from(path),
    }
}

fn cite_command(ctx: &EvidenceContext<'_>) -> String {
    format!(
        "produce the bundle under {dir}/qa/ ({CONTRACT_REFERENCE}), then `task action=notes id={task} note_type=platform_proof notes=\"{BUNDLE_CITATION} {dir}/qa/bundle.json\"`",
        dir = ctx.task_artifacts_dir.display(),
        task = ctx.task_id,
    )
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Committer time (unix seconds) of a commit.
pub fn committer_time(repo: &Path, commit: &str) -> Option<i64> {
    git(repo, &["show", "-s", "--format=%ct", commit])?
        .parse()
        .ok()
}

fn is_full_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn mtime_secs(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|duration| duration.as_secs() as i64)
}

/// Validate the cited cas-c3b8 bundle for the delivered head.
pub fn validate_bundle(ctx: &EvidenceContext<'_>) -> Result<BundleReceipt, EvidenceRefusal> {
    let cited = cited_bundle_path(ctx.notes).ok_or_else(|| {
        EvidenceRefusal::new(
            format!("not cited (no `{BUNDLE_CITATION}` note)"),
            cite_command(ctx),
        )
    })?;
    let manifest_path = resolve_inside_task_dir(ctx, &cited)?;
    let bundle_dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| {
            EvidenceRefusal::new(
                format!("cited at {cited}, which has no directory"),
                cite_command(ctx),
            )
        })?;

    let raw = std::fs::read_to_string(&manifest_path).map_err(|error| {
        EvidenceRefusal::new(
            format!("unreadable at {} ({error})", manifest_path.display()),
            cite_command(ctx),
        )
    })?;
    let manifest: Manifest = serde_json::from_str(&raw).map_err(|error| {
        EvidenceRefusal::new(
            format!(
                "malformed: {} does not parse as a v1 manifest ({error})",
                manifest_path.display()
            ),
            format!("rewrite bundle.json to the v1 shape in {CONTRACT_REFERENCE}"),
        )
    })?;
    if manifest.schema != 1 {
        return Err(EvidenceRefusal::new(
            format!("schema {} (only schema 1 is understood)", manifest.schema),
            format!("regenerate bundle.json per {CONTRACT_REFERENCE}"),
        ));
    }
    if manifest.task_id != ctx.task_id {
        return Err(EvidenceRefusal::new(
            format!(
                "for task {} (bundle.json task_id), not {}",
                manifest.task_id, ctx.task_id
            ),
            cite_command(ctx),
        ));
    }
    if !matches!(manifest.producer.as_str(), "cas-qa-craft" | "journey") {
        return Err(EvidenceRefusal::new(
            format!(
                "from producer {:?}; the implementer's close needs a cas-qa-craft or journey bundle",
                manifest.producer
            ),
            cite_command(ctx),
        ));
    }
    // The contract lets a `journey` bundle skip polish because the release
    // journey evaluation scores it. Supervisor decision (cas-0cd5): that
    // exception does not reach a delivery close; a delivery still needs its
    // own polish proof.
    if manifest.producer == "journey"
        && !matches!(manifest.visual_qa_status.as_str(), "pass" | "scoped")
    {
        return Err(EvidenceRefusal::new(
            format!(
                "a journey bundle without polish proof (visual_qa_status {:?}); the journey polish exception covers the release evaluation, not a delivery close",
                manifest.visual_qa_status
            ),
            format!(
                "add the polish keys to {} or cite a cas-qa-craft bundle: {}",
                manifest_path.display(),
                producing_command("visual_qa_stdout", &bundle_dir)
            ),
        ));
    }

    // cas-a6ab: a deployed-origin run names its reason and a configured
    // origin up front; its commit binding and proof are checked below.
    let deployed_origin = match manifest.deployed.as_ref() {
        Some(deployed) => Some(check_deployed_declaration(ctx, deployed, &manifest_path)?),
        None => None,
    };

    // 2. Required files.
    let files = &manifest.files;
    let singles: [(&str, &Option<String>); 9] = [
        ("trace", &files.trace),
        ("trace_actions", &files.trace_actions),
        ("receipt", &files.receipt),
        ("aria_yaml", &files.aria_yaml),
        ("aria_json", &files.aria_json),
        ("visual_qa", &files.visual_qa),
        ("visual_qa_json", &files.visual_qa_json),
        ("visual_qa_stdout", &files.visual_qa_stdout),
        ("critique", &files.critique),
    ];
    let mut listed: Vec<(String, PathBuf)> = Vec::new();
    for (key, value) in singles {
        let Some(relative) = value.as_deref().filter(|value| !value.trim().is_empty()) else {
            return Err(missing_key(key));
        };
        listed.push((
            key.to_string(),
            resolve_bundle_file(&bundle_dir, key, relative)?,
        ));
    }
    if files.cells.is_empty() {
        return Err(missing_key("cells"));
    }
    for (index, relative) in files.cells.iter().enumerate() {
        listed.push((
            format!("cells[{index}]"),
            resolve_bundle_file(&bundle_dir, "cells", relative)?,
        ));
    }
    for suffix in POLISH_RENDERS {
        if !files
            .polish_screenshots
            .iter()
            .any(|path| path.ends_with(suffix))
        {
            return Err(EvidenceRefusal::new(
                format!("missing the polish render `*{suffix}` in files.polish_screenshots"),
                producing_command("polish_screenshots", &bundle_dir),
            ));
        }
    }
    for (index, relative) in files.polish_screenshots.iter().enumerate() {
        listed.push((
            format!("polish_screenshots[{index}]"),
            resolve_bundle_file(&bundle_dir, "polish_screenshots", relative)?,
        ));
    }
    if manifest.visual_change {
        for mode in A11Y_MODES {
            if !files.a11y.iter().any(|path| path.contains(mode)) {
                return Err(EvidenceRefusal::new(
                    format!("missing the `{mode}` capture in files.a11y (visual_change is true)"),
                    producing_command("a11y", &bundle_dir),
                ));
            }
        }
    }
    for (index, relative) in files.a11y.iter().enumerate() {
        listed.push((
            format!("a11y[{index}]"),
            resolve_bundle_file(&bundle_dir, "a11y", relative)?,
        ));
    }
    if let Some(deployed) = manifest.deployed.as_ref() {
        listed.push((
            "deployed.deployment_proof".to_string(),
            resolve_bundle_file(
                &bundle_dir,
                "deployed.deployment_proof",
                deployed.deployment_proof.trim(),
            )?,
        ));
    }
    for (key, path) in &listed {
        let key_root = key.split('[').next().unwrap_or(key);
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_file() && meta.len() > 0 => {}
            Ok(meta) if meta.is_file() => {
                return Err(EvidenceRefusal::new(
                    format!("incomplete: files.{key} ({}) is empty", path.display()),
                    producing_command(key_root, &bundle_dir),
                ));
            }
            _ => {
                return Err(EvidenceRefusal::new(
                    format!(
                        "incomplete: files.{key} ({}) does not exist",
                        path.display()
                    ),
                    producing_command(key_root, &bundle_dir),
                ));
            }
        }
    }

    // 3. Staleness against the delivered head.
    let rerun = format!(
        "re-run the cas-qa-craft worked example against {} and rewrite {}",
        short(ctx.delivered_head),
        manifest_path.display()
    );
    if !is_full_sha(&manifest.head_sha) {
        return Err(EvidenceRefusal::new(
            format!(
                "unbound: head_sha {:?} is not a full 40-hex commit",
                manifest.head_sha
            ),
            rerun,
        ));
    }
    let delivered_time = committer_time(ctx.repo, ctx.delivered_head).ok_or_else(|| {
        EvidenceRefusal::new(
            format!(
                "uncheckable: the delivered head {} is not readable in {}",
                short(ctx.delivered_head),
                ctx.repo.display()
            ),
            "push the delivery commit, then retry close".to_string(),
        )
    })?;
    // A documented rebind preserves the execution revision. Only unchanged
    // product/journey inputs across documentation-only commits can reuse it.
    let delivered_time = if manifest.executed_head_sha.is_empty() {
        delivered_time
    } else if journeys::doc_only_rebind(ctx.repo, &manifest.executed_head_sha, &manifest.head_sha) {
        committer_time(ctx.repo, &manifest.executed_head_sha).ok_or_else(||
            EvidenceRefusal::new("executed QA revision is unreadable", rerun.clone()))?
    } else {
        return Err(EvidenceRefusal::new("QA evidence rebind changed product or journey inputs", rerun));
    };
    let head_covers_delivery = manifest.head_sha == ctx.delivered_head
        || git(
            ctx.repo,
            &[
                "merge-base",
                "--is-ancestor",
                ctx.delivered_head,
                &manifest.head_sha,
            ],
        )
        .is_some();
    if !head_covers_delivery {
        return Err(EvidenceRefusal::new(
            format!(
                "stale: it was recorded for {} but the delivery is {} (a commit landed after the bundle, or the bundle is for another build)",
                short(&manifest.head_sha),
                short(ctx.delivered_head)
            ),
            rerun,
        ));
    }
    if let Some(deployed) = manifest.deployed.as_ref() {
        check_deployed_binding(ctx, deployed, &listed, &rerun)?;
    }
    let created = chrono::DateTime::parse_from_rfc3339(manifest.created_at.trim())
        .map(|time| time.timestamp())
        .map_err(|_| {
            EvidenceRefusal::new(
                format!(
                    "unbound: created_at {:?} is not RFC 3339",
                    manifest.created_at
                ),
                rerun.clone(),
            )
        })?;
    if created < delivered_time {
        return Err(EvidenceRefusal::new(
            format!(
                "stale: created_at {} is older than the delivered commit {}",
                manifest.created_at,
                short(ctx.delivered_head)
            ),
            rerun,
        ));
    }
    for (key, path) in &listed {
        if mtime_secs(path).is_some_and(|mtime| mtime < delivered_time) {
            return Err(EvidenceRefusal::new(
                format!(
                    "stale: files.{key} ({}) predates the delivered commit {}",
                    path.display(),
                    short(ctx.delivered_head)
                ),
                rerun,
            ));
        }
    }

    // 4. Trace proves a passing assertion and no failing one.
    let path_of = |key: &str| {
        listed
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, path)| path.clone())
    };
    let trace_path = path_of("trace").unwrap_or_default();
    let summary = trace_expect_summary(&trace_path).map_err(|error| {
        EvidenceRefusal::new(
            format!("unusable: files.trace {error}"),
            producing_command("trace", &bundle_dir),
        )
    })?;
    if summary.failed > 0 {
        return Err(EvidenceRefusal::new(
            format!(
                "a failing run: files.trace records {} failed Expect step(s) — evidence must come from a passing run",
                summary.failed
            ),
            format!(
                "fix the failure, then {}",
                producing_command("trace", &bundle_dir)
            ),
        ));
    }
    if summary.passed == 0 {
        return Err(EvidenceRefusal::new(
            "assertion-free: files.trace contains no passing Expect step",
            producing_command("trace", &bundle_dir),
        ));
    }
    let actions =
        std::fs::read_to_string(path_of("trace_actions").unwrap_or_default()).unwrap_or_default();
    if !actions.lines().any(|line| line.contains("Expect \"")) {
        return Err(EvidenceRefusal::new(
            "incomplete: files.trace_actions lists no `Expect \"` step",
            producing_command("trace_actions", &bundle_dir),
        ));
    }

    // 5. Polish run passed, or (cas-e371, GH #1023 finding 1) introduced no
    // finding the base build did not already have. A narrow delivery on a
    // page with an older visual backlog is judged on what it changed; the
    // backlog is the base run's, and the page's follow-up work.
    let visual_qa_json = path_of("visual_qa_json").unwrap_or_default();
    let delivered_label = format!("the delivered commit {}", short(ctx.delivered_head));
    let run_command = || {
        producing_command("visual_qa_json", &bundle_dir).replace(
            "<url>",
            "<a local URL serving a build of the delivered commit>",
        )
    };
    match manifest.visual_qa_status.as_str() {
        // Markdown and stdout are retained as evidence, but their presentation
        // varies by script and source count. The JSON run report is the verdict
        // authority (GH #1017/#1025); no hand-written PASS markers are needed.
        "pass" => check_visual_qa_run_at(
            &visual_qa_json,
            delivered_time,
            &delivered_label,
            deployed_origin.as_deref(),
        )
        .map_err(|problem| EvidenceRefusal::new(problem, run_command()))?,
        "scoped" => {
            let Some(relative) = files
                .visual_qa_baseline_json
                .as_deref()
                .filter(|value| !value.trim().is_empty())
            else {
                return Err(missing_key("visual_qa_baseline_json"));
            };
            let baseline =
                resolve_bundle_file(&bundle_dir, "visual_qa_baseline_json", relative)?;
            check_visual_qa_scoped_at(
                &visual_qa_json,
                &baseline,
                delivered_time,
                &delivered_label,
                deployed_origin.as_deref(),
            )
                .map_err(|problem| {
                    EvidenceRefusal::new(
                        problem,
                        producing_command("visual_qa_baseline_json", &bundle_dir),
                    )
                })?;
        }
        other => {
            return Err(EvidenceRefusal::new(
                format!(
                    "missing polish proof: visual_qa_status is {other:?} (\"pass\", or \"scoped\" with a base-build run, closes without a supervisor override)"
                ),
                producing_command("visual_qa_stdout", &bundle_dir),
            ));
        }
    }

    // cas-a6ab: an authenticated run must not carry its credentials.
    if manifest.deployed.is_some() {
        check_no_secrets(&bundle_dir, &listed)?;
    }

    // 6. Critique floor.
    for dimension in RUBRIC {
        match manifest.critique_score.get(dimension) {
            Some(score) if (0..=5).contains(score) => {}
            _ => {
                return Err(EvidenceRefusal::new(
                    format!("missing the critique score for `{dimension}` (0–5)"),
                    producing_command("critique", &bundle_dir),
                ));
            }
        }
    }
    let below: Vec<String> = RUBRIC
        .iter()
        .filter_map(|dimension| {
            let score = manifest.critique_score[*dimension];
            let floor = if FLOOR_DIMENSIONS.contains(dimension) {
                FLOOR as i64
            } else {
                1
            };
            (score < floor).then(|| format!("{dimension}={score} (floor {floor})"))
        })
        .collect();
    if !below.is_empty() {
        return Err(EvidenceRefusal::new(
            format!("below the polish floor: {}", below.join(", ")),
            "improve the surface, re-score it with the cas-ui-craft rubric, and rewrite critique.md and critique_score".to_string(),
        ));
    }

    Ok(BundleReceipt {
        manifest: manifest_path,
        head_sha: manifest.head_sha,
        passed_expects: summary.passed,
    })
}

/// The report a visual-QA script writes (`visual-qa.json`). A bundle's
/// `visual_qa_status` is only a claim; this is the run's own record.
#[derive(Debug, Deserialize)]
struct VisualQaRun {
    /// Every unsuppressed finding, as visual-qa.mjs records it (type, rule,
    /// semantic/text geometry or legacy selector, url, scheme, viewport). A scoped comparison
    /// needs it; the pass check does not.
    #[serde(default)]
    findings: Option<Vec<serde_json::Value>>,
    #[serde(default)]
    status: String,
    #[serde(default, rename = "generatedAt")]
    generated_at: String,
    #[serde(default)]
    urls: Vec<String>,
    #[serde(default)]
    input: String,
    #[serde(default, rename = "totalIssues")]
    total_issues: Option<usize>,
    #[serde(default, rename = "validRenders")]
    valid_renders: Option<usize>,
    #[serde(default)]
    renders: Vec<VisualQaRender>,
    /// Older canonical reports omit this field, so only an explicit `false`
    /// is refused there. A single-source report must record `true`.
    #[serde(default)]
    strict: Option<bool>,
}

#[derive(Debug, Deserialize)]
struct VisualQaRender {
    #[serde(default)]
    valid: bool,
    /// GH #1166: a project visual-qa.mjs records each render's unsuppressed
    /// findings here (already annotated with viewport and scheme) instead of
    /// a top-level `findings` list.
    #[serde(default)]
    issues: Option<Vec<serde_json::Value>>,
}

impl VisualQaRun {
    /// The findings a scoped comparison pairs. The canonical report's
    /// top-level `findings` list wins. Otherwise a per-render report
    /// (`renders[].issues`, GH #1166) is flattened, each issue given the
    /// report's page (`input`) when it names none. `None` when the report
    /// carries neither shape.
    fn comparable_findings(&self) -> Option<Vec<serde_json::Value>> {
        if let Some(findings) = &self.findings {
            return Some(findings.clone());
        }
        if self.renders.is_empty() || self.renders.iter().any(|render| render.issues.is_none()) {
            return None;
        }
        let page = self.input.trim();
        Some(
            self.renders
                .iter()
                .flat_map(|render| render.issues.iter().flatten())
                .map(|issue| {
                    let mut issue = issue.clone();
                    if let Some(object) = issue.as_object_mut()
                        && !page.is_empty()
                        && object.get("url").and_then(serde_json::Value::as_str).is_none_or(str::is_empty)
                    {
                        object.insert("url".into(), serde_json::Value::String(page.to_string()));
                    }
                    issue
                })
                .collect(),
        )
    }
}

/// Whether a visual-QA target is a local build: loopback or unspecified
/// hosts, `*.localhost`, a `file:` page, or a bare path (visual-qa.mjs opens
/// a target without `scheme://` as a local file). A production or other
/// remote origin serves whatever is deployed there, not the delivered commit.
pub fn is_local_origin(target: &str) -> bool {
    let target = target.trim();
    if !target.contains("://") {
        return !target.is_empty();
    }
    let Ok(parsed) = url::Url::parse(target) else {
        return false;
    };
    match parsed.scheme() {
        "file" => true,
        "http" | "https" => match parsed.host() {
            Some(url::Host::Domain(domain)) => {
                let domain = domain.trim_end_matches('.').to_ascii_lowercase();
                domain == "localhost" || domain.ends_with(".localhost")
            }
            Some(url::Host::Ipv4(ip)) => ip.is_loopback() || ip.is_unspecified(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback() || ip.is_unspecified(),
            None => false,
        },
        _ => false,
    }
}

/// cas-a6a3 (GH #1007): a claimed visual-QA pass counts only when the strict
/// run's own report exists, says PASS, was generated no earlier than
/// `not_before` (unix seconds; the delivered commit, or the QA round's
/// opening), and ran against local builds only. The field report behind this:
/// a bundle claimed `visual_qa_status: pass` while its ledger said the script
/// was unavailable, and a later strict run pointed at the production URL.
/// Returns the reason the claim is refused.
pub fn check_visual_qa_run(
    report: &Path,
    not_before: i64,
    not_before_label: &str,
) -> Result<(), String> {
    check_visual_qa_run_at(report, not_before, not_before_label, None)
}

/// [`check_visual_qa_run`], also accepting targets on `deployed_origin`
/// (cas-a6ab) when the bundle's deployed-origin declaration was validated.
pub fn check_visual_qa_run_at(
    report: &Path,
    not_before: i64,
    not_before_label: &str,
    deployed_origin: Option<&str>,
) -> Result<(), String> {
    let claim = "claims a visual-QA pass, but";
    let run = read_visual_qa_run(report, claim)?;
    let legacy_single_source = run.status.is_empty() && run.total_issues.is_some();
    if legacy_single_source {
        if run.total_issues != Some(0)
            || run.renders.is_empty()
            || run.valid_renders != Some(run.renders.len())
            || run.renders.iter().any(|render| !render.valid)
            || run.strict != Some(true)
        {
            return Err(format!(
                "{claim} {} records totalIssues={:?}, validRenders={:?}/{}, strict={:?}; a single-source pass needs zero issues and every render valid under --strict",
                report.display(), run.total_issues, run.valid_renders, run.renders.len(), run.strict
            ));
        }
    } else if run.status != "PASS" || run.total_issues.is_some_and(|issues| issues > 0) {
        return Err(format!(
            "{claim} {} records status {:?} and totalIssues {:?}, not a passing run",
            report.display(),
            run.status,
            run.total_issues
        ));
    }
    check_run_provenance(
        &run,
        report,
        claim,
        Some((not_before, not_before_label)),
        deployed_origin,
    )?;
    Ok(())
}

fn read_visual_qa_run(report: &Path, claim: &str) -> Result<VisualQaRun, String> {
    let raw = std::fs::read_to_string(report).map_err(|_| {
        format!(
            "{claim} the strict run's own report {} is missing or unreadable",
            report.display()
        )
    })?;
    serde_json::from_str(&raw).map_err(|error| {
        format!(
            "{claim} {} is not the JSON report visual-qa.mjs writes ({error})",
            report.display()
        )
    })
}

/// The run was strict, (when `fresh` is given) generated no earlier than
/// that time, and checked local builds only. Returns the targets it checked.
fn check_run_provenance(
    run: &VisualQaRun,
    report: &Path,
    claim: &str,
    fresh: Option<(i64, &str)>,
    deployed_origin: Option<&str>,
) -> Result<Vec<String>, String> {
    if run.strict == Some(false) {
        return Err(format!(
            "{claim} {} records a run without --strict",
            report.display()
        ));
    }
    if let Some((not_before, not_before_label)) = fresh {
        let generated = chrono::DateTime::parse_from_rfc3339(run.generated_at.trim())
            .map(|time| time.timestamp())
            .map_err(|_| {
                format!(
                    "{claim} {} records no valid generatedAt ({:?}), so nothing shows when the run happened",
                    report.display(),
                    run.generated_at
                )
            })?;
        if generated < not_before {
            return Err(format!(
                "{claim} {} was generated at {}, before {not_before_label}: that run did not check this build",
                report.display(),
                run.generated_at.trim()
            ));
        }
    }
    let targets: Vec<String> = if !run.urls.is_empty() {
        run.urls.clone()
    } else if !run.input.trim().is_empty() {
        vec![run.input.trim().to_string()]
    } else {
        Vec::new()
    };
    if targets.is_empty() {
        return Err(format!(
            "{claim} {} names no URL it checked",
            report.display()
        ));
    }
    if let Some(remote) = targets.iter().find(|target| {
        !is_local_origin(target)
            && !deployed_origin.is_some_and(|origin| url_origin(target).as_deref() == Some(origin))
    }) {
        return Err(format!(
            "{claim} {} ran against {remote}, which is not a local build of the delivered commit (a production or remote origin shows what is deployed there, not this commit)",
            report.display()
        ));
    }
    Ok(targets)
}

/// The page a visual-QA target names, without the origin: the delivered and
/// the base build are served on different local ports.
fn visual_qa_page(target: &str) -> String {
    match url::Url::parse(target.trim()) {
        Ok(parsed) if parsed.scheme() != "file" => {
            let mut page = parsed.path().to_string();
            if let Some(query) = parsed.query() {
                page.push('?');
                page.push_str(query);
            }
            page
        }
        _ => target.trim().to_string(),
    }
}

/// Placeholder a per-render random id fragment is compared as.
const RANDOM_ID_PLACEHOLDER: &str = "<uuid>";

/// cas-7c15 (GH #1078): `text` with every UUID-shaped fragment
/// (8-4-4-4-12 hex digits, either case) replaced by [`RANDOM_ID_PLACEHOLDER`].
/// Frameworks mint such ids per render (Quasar's `#f_<uuid>` focus inputs),
/// so the same element carries a different id in the delivered and the base
/// run. A fragment glued to further hex digits is not UUID-shaped and stays.
fn normalize_random_ids(text: &str) -> String {
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    const LEN: usize = 36;
    let bytes = text.as_bytes();
    let is_uuid_at = |start: usize| {
        if start + LEN > bytes.len() {
            return false;
        }
        let mut at = start;
        for (index, group) in GROUPS.iter().enumerate() {
            if index > 0 {
                if bytes[at] != b'-' {
                    return false;
                }
                at += 1;
            }
            if !bytes[at..at + group].iter().all(u8::is_ascii_hexdigit) {
                return false;
            }
            at += group;
        }
        let before_ok = start == 0 || !bytes[start - 1].is_ascii_hexdigit();
        let after_ok = at == bytes.len() || !bytes[at].is_ascii_hexdigit();
        before_ok && after_ok
    };
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut index = 0;
    while index < bytes.len() {
        if is_uuid_at(index) {
            out.push_str(&text[copied..index]);
            out.push_str(RANDOM_ID_PLACEHOLDER);
            index += LEN;
            copied = index;
        } else {
            index += 1;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Finding context is independent of DOM class names. Rule/reason and render
/// state stay separate so a new defect on a renamed element is still added.
fn visual_qa_finding_text(finding: &serde_json::Value, field: &str) -> String {
    finding
        .get(field)
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn visual_qa_finding_context(finding: &serde_json::Value) -> String {
    serde_json::json!([
        visual_qa_finding_text(finding, "type"),
        visual_qa_finding_text(finding, "rule"),
        visual_qa_finding_text(finding, "reason"),
        visual_qa_page(&visual_qa_finding_text(finding, "url")),
        visual_qa_finding_text(finding, "scheme"),
        visual_qa_finding_text(finding, "state"),
        finding.get("viewport")
    ])
    .to_string()
}

fn visual_qa_finding_path(finding: &serde_json::Value, field: &str) -> String {
    // Whitespace inside quoted CSS attribute values is significant.
    normalize_random_ids(finding.get(field).and_then(serde_json::Value::as_str)
        .unwrap_or_default())
}

fn visual_qa_selector(finding: &serde_json::Value) -> String {
    let selector = visual_qa_finding_path(finding, "selector");
    if selector.is_empty() {
        visual_qa_finding_path(finding, "elementPath")
    } else {
        selector
    }
}

/// Newer producers may report textBounds or box. Historical canonical clipping
/// reports carry textSample plus ancestorBox (the text's clipping rectangle).
/// Never use an incomplete/non-finite/negative box as identity evidence.
fn visual_qa_finding_bounds(finding: &serde_json::Value, field: &str) -> Option<[f64; 4]> {
    let bounds = finding.get(field)?;
    let coordinates = ["x", "y", "width", "height"]
        .map(|field| bounds.get(field).and_then(serde_json::Value::as_f64));
    let [Some(x), Some(y), Some(width), Some(height)] = coordinates else {
        return None;
    };
    let bounds = [x, y, width, height];
    (bounds.iter().all(|value| value.is_finite()) && width > 0.0 && height > 0.0).then_some(bounds)
}

/// Half a CSS pixel allows subpixel report rounding (cas-a286: 34.48 vs 34.50),
/// below the canonical inspector's 1px BOX_TOLERANCE. It is not a defect waiver:
/// text, rule, viewport and scheme must agree as well.
const VISUAL_QA_IDENTITY_BOUNDS_TOLERANCE: f64 = 0.5;

fn visual_qa_same_bounds(left: [f64; 4], right: [f64; 4]) -> bool {
    left.into_iter()
        .zip(right)
        .all(|(left, right)| (left - right).abs() <= VISUAL_QA_IDENTITY_BOUNDS_TOLERANCE)
}

/// Compare the same kind of rectangle across report versions. A new textBounds
/// field must not be compared to an old clipping ancestor's different box.
fn visual_qa_common_bounds(left: &serde_json::Value, right: &serde_json::Value) -> Option<bool> {
    visual_qa_bounds_pair(left, right).map(|(left, right)| visual_qa_same_bounds(left, right))
}

fn visual_qa_bounds_pair(
    left: &serde_json::Value,
    right: &serde_json::Value,
) -> Option<([f64; 4], [f64; 4])> {
    ["textBounds", "box", "ancestorBox"]
        .into_iter()
        .find_map(|field| {
            Some((
                visual_qa_finding_bounds(left, field)?,
                visual_qa_finding_bounds(right, field)?,
            ))
        })
}

/// Measured defect details the producer records. A translated finding must
/// carry identical values, so a moved element whose defect changed is new.
const VISUAL_QA_TRANSLATION_METRICS: [&str; 5] =
    ["ratio", "threshold", "foreground", "background", "largeText"];

/// cas-5488 (GH #1152): content inserted above an element moves it without
/// changing it. Pair a pure translation only when the exact DOM path, box
/// size and measured defect all agree; a renamed or resized element still
/// needs the strict position match above.
fn visual_qa_translated_element(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    let selector = visual_qa_selector(left);
    if selector.is_empty() || selector != visual_qa_selector(right) {
        return false;
    }
    let Some((left_bounds, right_bounds)) = visual_qa_bounds_pair(left, right) else {
        return false;
    };
    let same_size = [2, 3].into_iter().all(|index| {
        (left_bounds[index] - right_bounds[index]).abs() <= VISUAL_QA_IDENTITY_BOUNDS_TOLERANCE
    });
    same_size
        && VISUAL_QA_TRANSLATION_METRICS
            .iter()
            .all(|field| left.get(field) == right.get(field))
}

fn visual_qa_same_element(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    let semantic = |finding: &serde_json::Value| {
        let role = visual_qa_finding_text(finding, "role");
        let name = visual_qa_finding_text(finding, "accessibleName");
        (!role.is_empty() && !name.is_empty()).then_some((role, name))
    };
    if let (Some(left_id), Some(right_id)) = (semantic(left), semantic(right)) {
        if left_id != right_id {
            return false;
        }
        return visual_qa_common_bounds(left, right).unwrap_or(true)
            || visual_qa_translated_element(left, right);
    }
    let left_text = visual_qa_finding_text(left, "textSample");
    let right_text = visual_qa_finding_text(right, "textSample");
    if !left_text.is_empty() && !right_text.is_empty() {
        if left_text != right_text {
            return false;
        }
        if let Some(same_bounds) = visual_qa_common_bounds(left, right) {
            return same_bounds || visual_qa_translated_element(left, right);
        }
    }
    // Older minimal reports do not identify text or accessible elements. Retain
    // exact selectors (including UUID normalization) only for that fallback;
    // never erase classes or let this override a stable identity mismatch.
    let selector = visual_qa_selector(left);
    !selector.is_empty() && selector == visual_qa_selector(right)
}

fn visual_qa_findings_match(left: &serde_json::Value, right: &serde_json::Value) -> bool {
    visual_qa_finding_context(left) == visual_qa_finding_context(right)
        && visual_qa_same_element(left, right)
        // Overlap findings must identify the same second element too. Legacy
        // reports have its selector, not enough evidence to pair its rename.
        && visual_qa_finding_path(left, "otherElementPath")
            == visual_qa_finding_path(right, "otherElementPath")
        && visual_qa_finding_text(left, "otherTextSample")
            == visual_qa_finding_text(right, "otherTextSample")
}

/// Reserve each baseline finding once. Geometry tolerance may produce multiple
/// candidates, so an augmenting path avoids a greedy/order-dependent refusal.
fn pair_visual_qa_findings(
    tip: &[serde_json::Value],
    baseline: &[serde_json::Value],
) -> Vec<usize> {
    let candidates: Vec<Vec<usize>> = tip
        .iter()
        .map(|finding| {
            baseline
                .iter()
                .enumerate()
                .filter_map(|(index, base)| {
                    visual_qa_findings_match(finding, base).then_some(index)
                })
                .collect()
        })
        .collect();
    fn assign(
        index: usize,
        candidates: &[Vec<usize>],
        owners: &mut [Option<usize>],
        visited: &mut [bool],
    ) -> bool {
        for &base in &candidates[index] {
            if visited[base] {
                continue;
            }
            visited[base] = true;
            let previous = owners[base];
            let paired = match previous {
                None => true,
                Some(previous) => assign(previous, candidates, owners, visited),
            };
            if paired {
                owners[base] = Some(index);
                return true;
            }
        }
        false
    }
    let mut owners = vec![None; baseline.len()];
    let mut introduced = Vec::new();
    for index in 0..tip.len() {
        if !assign(
            index,
            &candidates,
            &mut owners,
            &mut vec![false; baseline.len()],
        ) {
            introduced.push(index);
        }
    }
    introduced
}

/// Selectors are retained in diagnostics so the reviewer can locate added
/// findings, but no longer govern pairing when stable identity is available.
fn visual_qa_finding_key(finding: &serde_json::Value) -> String {
    format!(
        "{} | {} | {}",
        visual_qa_finding_context(finding),
        visual_qa_selector(finding),
        visual_qa_finding_path(finding, "otherElementPath")
    )
}

/// A scoped visual-QA check that passed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopedVisualQa {
    /// Findings of the delivered build, all of which the base build has.
    pub pre_existing: usize,
}

/// cas-e371 (GH #1023 finding 1): the close gate measures what a delivery
/// changed, not the page's whole visual backlog. `tip` is the strict run of
/// the delivered build (fresh, local), `baseline` the strict run of the base
/// build over the same pages (local). The check passes when every finding of
/// the delivered build also appears in the base run, so the delivery
/// introduced none; it fails naming the findings the base build does not
/// have. The pre-existing ones are the page's follow-up work.
pub fn check_visual_qa_scoped(
    tip: &Path,
    baseline: &Path,
    not_before: i64,
    not_before_label: &str,
) -> Result<ScopedVisualQa, String> {
    check_visual_qa_scoped_at(tip, baseline, not_before, not_before_label, None)
}

/// [`check_visual_qa_scoped`], also accepting targets on `deployed_origin`
/// (cas-a6ab). The base run stays local: it serves the base commit.
pub fn check_visual_qa_scoped_at(
    tip: &Path,
    baseline: &Path,
    not_before: i64,
    not_before_label: &str,
    deployed_origin: Option<&str>,
) -> Result<ScopedVisualQa, String> {
    let claim = "claims a scoped visual-QA result, but";
    let tip_run = read_visual_qa_run(tip, claim)?;
    let tip_targets = check_run_provenance(
        &tip_run,
        tip,
        claim,
        Some((not_before, not_before_label)),
        deployed_origin,
    )?;
    let base_run = read_visual_qa_run(baseline, claim)?;
    let base_targets = check_run_provenance(&base_run, baseline, claim, None, None)?;
    let (Some(tip_findings), Some(base_findings)) =
        (tip_run.comparable_findings(), base_run.comparable_findings())
    else {
        return Err(format!(
            "{claim} {} and {} must both record their findings, as a top-level `findings` list (the builtin visual-qa.mjs) or per render as `renders[].issues` (a per-source project visual-qa.mjs), so the two runs can be compared",
            tip.display(),
            baseline.display()
        ));
    };
    let (tip_findings, base_findings) = (&tip_findings, &base_findings);
    let base_pages: std::collections::BTreeSet<String> =
        base_targets.iter().map(|target| visual_qa_page(target)).collect();
    let missing: Vec<String> = tip_targets
        .iter()
        .map(|target| visual_qa_page(target))
        .filter(|page| !base_pages.contains(page))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "{claim} the base run {} did not check {}; run it over the same pages as the delivered build",
            baseline.display(),
            missing.join(", ")
        ));
    }
    let introduced: Vec<String> = pair_visual_qa_findings(tip_findings, base_findings)
        .into_iter()
        .map(|index| visual_qa_finding_key(&tip_findings[index]))
        .collect();
    if !introduced.is_empty() {
        let shown: Vec<&str> = introduced.iter().take(5).map(String::as_str).collect();
        return Err(format!(
            "the delivery introduced {} visual-QA finding(s) the base build does not have ({}): {}{}",
            introduced.len(),
            tip.display(),
            shown.join("; "),
            if introduced.len() > shown.len() { "; …" } else { "" }
        ));
    }
    Ok(ScopedVisualQa {
        pre_existing: tip_findings.len(),
    })
}

/// cas-a6ab: the declaration half of a deployed-origin run: a recorded
/// reason, and an origin that is remote and configured for this project.
/// Returns the normalised origin.
fn check_deployed_declaration(
    ctx: &EvidenceContext<'_>,
    deployed: &DeployedEvidence,
    manifest_path: &Path,
) -> Result<String, EvidenceRefusal> {
    let fix = format!(
        "record deployed.origin, deployed.reason, deployed.deployed_sha and deployed.deployment_proof in {} ({CONTRACT_REFERENCE})",
        manifest_path.display()
    );
    if deployed.reason.trim().chars().count() < DEPLOYED_REASON_MIN_CHARS {
        return Err(EvidenceRefusal::new(
            "a deployed-origin run without its reason: deployed.reason must say why a local build could not be used (for example the backend's CORS rejecting localhost on its session endpoint)",
            fix,
        ));
    }
    let Some(origin) = url_origin(&deployed.origin) else {
        return Err(EvidenceRefusal::new(
            format!(
                "a deployed-origin run whose deployed.origin {:?} is not an http(s) origin",
                deployed.origin
            ),
            fix,
        ));
    };
    if is_local_origin(&origin) {
        return Err(EvidenceRefusal::new(
            format!("a deployed-origin run that names a local origin ({origin}); a local build needs no deployed declaration"),
            "remove `deployed` from bundle.json".to_string(),
        ));
    }
    let allowed: Vec<String> = ctx
        .deployed_origins
        .iter()
        .filter_map(|configured| url_origin(configured))
        .collect();
    if !allowed.iter().any(|configured| *configured == origin) {
        return Err(EvidenceRefusal::new(
            format!(
                "from the wrong origin: {origin} is not a configured deployed origin (qa.deployed_origins: {})",
                if allowed.is_empty() {
                    "none".to_string()
                } else {
                    allowed.join(", ")
                }
            ),
            format!(
                "run against a configured deployment, or have the supervisor add it: `cas config set qa.deployed_origins {origin}`"
            ),
        ));
    }
    Ok(origin)
}

/// cas-a6ab: the binding half of a deployed-origin run: the deployment
/// served the delivered commit (or a descendant of it), and the recorded
/// proof says so.
fn check_deployed_binding(
    ctx: &EvidenceContext<'_>,
    deployed: &DeployedEvidence,
    listed: &[(String, PathBuf)],
    rerun: &str,
) -> Result<(), EvidenceRefusal> {
    let sha = deployed.deployed_sha.trim();
    if !is_full_sha(sha) {
        return Err(EvidenceRefusal::new(
            format!("unbound: deployed.deployed_sha {sha:?} is not a full 40-hex commit"),
            rerun.to_string(),
        ));
    }
    let covers = sha == ctx.delivered_head
        || git(
            ctx.repo,
            &["merge-base", "--is-ancestor", ctx.delivered_head, sha],
        )
        .is_some();
    if !covers {
        return Err(EvidenceRefusal::new(
            format!(
                "stale: the deployment served {} but the delivery is {} (deploy the delivered commit, then re-run against it)",
                short(sha),
                short(ctx.delivered_head)
            ),
            rerun.to_string(),
        ));
    }
    let proof = listed
        .iter()
        .find(|(key, _)| key == "deployed.deployment_proof")
        .map(|(_, path)| path.clone())
        .unwrap_or_default();
    let recorded = std::fs::read_to_string(&proof).unwrap_or_default();
    if !recorded.contains(sha) {
        return Err(EvidenceRefusal::new(
            format!(
                "unproven: deployed.deployment_proof ({}) does not name the deployed commit {}",
                proof.display(),
                short(sha)
            ),
            "record what the deployment reports about itself (its version endpoint or deployment metadata, naming the full commit) into the proof file".to_string(),
        ));
    }
    Ok(())
}

/// Credential shapes an authenticated run can leak into its artifacts.
fn secret_patterns() -> &'static [(&'static str, regex::Regex)] {
    static PATTERNS: std::sync::OnceLock<Vec<(&'static str, regex::Regex)>> =
        std::sync::OnceLock::new();
    PATTERNS.get_or_init(|| {
        [
            (
                "a JSON web token",
                r"eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,}",
            ),
            ("a bearer token", r"(?i)\bbearer\s+[A-Za-z0-9._~+/=-]{16,}"),
            (
                "an API token",
                r"\b(?:gh[pousr]_[A-Za-z0-9]{20,}|sk_(?:live|test)_[A-Za-z0-9]{10,}|xox[abpr]-[A-Za-z0-9-]{10,})",
            ),
            (
                "a cookie or authorization header value",
                r#"(?i)"name"\s*:\s*"(?:cookie|set-cookie|authorization)"\s*,\s*"value"\s*:\s*(?P<json_value>"(?:\\.|[^"\\])*")"#,
            ),
            (
                "a cookie or authorization header value",
                r"(?im)^[\t ]*(?:cookie|set-cookie|authorization)[\t ]*:[\t ]*(?P<header_value>[^\r\n]*)",
            ),
            (
                "a saved browser storage state (cookies)",
                r#""cookies"\s*:\s*\[(?P<cookie_values>(?:[^"\]]|"(?:\\.|[^"\\])*")*)"#,
            ),
        ]
        .into_iter()
        .map(|(kind, pattern)| (kind, regex::Regex::new(pattern).expect("secret pattern")))
        .collect()
    })
}

/// Largest single text or trace entry the secret scan reads.
const SECRET_SCAN_MAX_BYTES: u64 = 64 * 1024 * 1024;

fn first_secret(text: &str) -> Option<&'static str> {
    secret_patterns()
        .iter()
        .find(|(_, pattern)| {
            pattern.captures_iter(text).any(|captures| {
                if let Some(value) = captures.name("json_value") {
                    json_value_has_secret(value.as_str())
                } else if let Some(value) = captures.name("header_value") {
                    // HTTP optional whitespace is outside the header value.
                    value_has_secret(value.as_str().trim_matches([' ', '\t']))
                } else if let Some(values) = captures.name("cookie_values") {
                    static VALUES: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
                    VALUES
                        .get_or_init(|| {
                            regex::Regex::new(r#""value"\s*:\s*("(?:\\.|[^"\\])*")"#)
                                .expect("cookie value pattern")
                        })
                        .captures_iter(values.as_str())
                        .any(|value| json_value_has_secret(&value[1]))
                } else {
                    // JWT, bearer and API token detection has no exemptions.
                    true
                }
            })
        })
        .map(|(kind, _)| *kind)
}

fn value_has_secret(value: &str) -> bool {
    value.chars().count() >= 8
        && !matches!(value, "REDACTED" | "[REDACTED]" | "<redacted>" | "***" | "")
}

fn json_value_has_secret(quoted_value: &str) -> bool {
    match serde_json::from_str::<String>(quoted_value) {
        Ok(value) => value_has_secret(&value),
        // Malformed JSON must not turn into a redaction exemption.
        Err(_) => quoted_value.trim_matches('"').chars().count() >= 8,
    }
}

/// cas-a6ab: an authenticated deployed run must not carry credentials into
/// the bundle. Scans every listed text artifact, and every text entry of the
/// trace (Playwright records request headers there), naming the file and
/// the kind of secret, never its value. A saved storage-state file anywhere
/// in the bundle is refused by name.
fn check_no_secrets(bundle_dir: &Path, listed: &[(String, PathBuf)]) -> Result<(), EvidenceRefusal> {
    let fix = "re-record without credentials: saved storage-state files and real cookie/authorization values of 8 or more characters are refused; redact each entire value to exactly REDACTED, [REDACTED], <redacted>, ***, or empty before listing the trace; JWT, bearer and API tokens remain refused".to_string();
    if let Ok(entries) = std::fs::read_dir(bundle_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
            if name.contains("storage-state") || name.contains("storagestate") {
                return Err(EvidenceRefusal::new(
                    format!(
                        "carrying credentials: {} is a saved browser storage state",
                        entry.path().display()
                    ),
                    fix,
                ));
            }
        }
    }
    for (key, path) in listed {
        let found = if key == "trace" {
            scan_trace_for_secrets(path)
        } else {
            scan_text_file_for_secrets(path)
        };
        if let Some((kind, place)) = found {
            return Err(EvidenceRefusal::new(
                format!(
                    "carrying credentials: files.{key} ({}{place}) contains {kind}",
                    path.display()
                ),
                fix,
            ));
        }
    }
    Ok(())
}

fn scan_text_file_for_secrets(path: &Path) -> Option<(&'static str, String)> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > SECRET_SCAN_MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    // Images and video are binary; only text can carry a copied header.
    let text = std::str::from_utf8(&bytes).ok()?;
    first_secret(text).map(|kind| (kind, String::new()))
}

fn scan_trace_for_secrets(path: &Path) -> Option<(&'static str, String)> {
    let file = std::fs::File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    for index in 0..archive.len() {
        let Ok(mut entry) = archive.by_index(index) else {
            continue;
        };
        if entry.size() > SECRET_SCAN_MAX_BYTES {
            continue;
        }
        let name = entry.name().to_string();
        let mut bytes = Vec::new();
        if entry.read_to_end(&mut bytes).is_err() {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        if let Some(kind) = first_secret(text) {
            return Some((kind, format!(" entry {name}")));
        }
    }
    None
}

fn missing_key(key: &str) -> EvidenceRefusal {
    EvidenceRefusal::new(
        format!("incomplete: bundle.json lists no files.{key}"),
        format!("produce `{key}` per {CONTRACT_REFERENCE} and list it in bundle.json"),
    )
}

fn short(sha: &str) -> &str {
    &sha[..sha.len().min(8)]
}

/// The exact command that produces a bundle key (contract §2).
pub fn producing_command(key: &str, bundle_dir: &Path) -> String {
    let dir = bundle_dir.display();
    match key {
        "trace" => format!(
            "run the cas-qa-craft evidence spec under the Playwright test runner with trace {{ mode: 'on', snapshots: {{ dom, aria, screen }} }} and copy its test-results trace.zip into {dir}; for a signed-in run, scrub it first with node <skills-dir>/cas-ui-craft/scripts/visual-qa.mjs --scrub-trace <raw trace.zip> {dir}/trace.zip ({CONTRACT_REFERENCE})"
        ),
        "receipt" | "aria_yaml" | "aria_json" | "cells" | "a11y" => format!(
            "run the cas-qa-craft evidence spec with trace snapshots {{ dom, aria, screen }} and write {key} into {dir} ({CONTRACT_REFERENCE})"
        ),
        "trace_actions" => format!(
            "cd {dir} && npx playwright trace open trace.zip && npx playwright trace actions > trace-actions.txt; npx playwright trace close"
        ),
        "polish_screenshots" | "visual_qa" | "visual_qa_json" | "visual_qa_stdout" => format!(
            "node <skills-dir>/cas-ui-craft/scripts/visual-qa.mjs --strict --artifact-dir {dir}/visual-qa <url> > {dir}/visual-qa.stdout 2>&1"
        ),
        "visual_qa_baseline_json" => format!(
            "serve a build of the base commit (git merge-base <target> <head>) and run node <skills-dir>/cas-ui-craft/scripts/visual-qa.mjs --strict --artifact-dir {dir}/visual-qa-baseline <the same local pages>, then list visual-qa-baseline/visual-qa.json as files.visual_qa_baseline_json"
        ),
        "critique" => format!(
            "score what the delivery changed with the cas-ui-craft rubric into {dir}/critique.md and bundle.json critique_score"
        ),
        _ => format!("produce `{key}` per {CONTRACT_REFERENCE}"),
    }
}

fn resolve_inside_task_dir(
    ctx: &EvidenceContext<'_>,
    cited: &str,
) -> Result<PathBuf, EvidenceRefusal> {
    let candidate = expand_home(cited);
    let resolved = candidate.canonicalize().map_err(|_| {
        EvidenceRefusal::new(
            format!("missing: the cited {cited} does not exist"),
            cite_command(ctx),
        )
    })?;
    let root = ctx.task_artifacts_dir.canonicalize().map_err(|_| {
        EvidenceRefusal::new(
            format!(
                "missing: the task artifacts dir {} does not exist",
                ctx.task_artifacts_dir.display()
            ),
            cite_command(ctx),
        )
    })?;
    if !resolved.starts_with(&root) {
        return Err(EvidenceRefusal::new(
            format!(
                "outside the task: {cited} resolves to {} which is not under {}",
                resolved.display(),
                root.display()
            ),
            cite_command(ctx),
        ));
    }
    if resolved
        .strip_prefix(&root)
        .ok()
        .and_then(|relative| relative.components().next())
        .is_some_and(|first| first.as_os_str() == "independent-qa")
    {
        return Err(EvidenceRefusal::new(
            "an independent QA round's bundle; the implementer's own evidence belongs in qa/ or journeys/<id>/",
            cite_command(ctx),
        ));
    }
    Ok(resolved)
}

fn resolve_bundle_file(
    bundle_dir: &Path,
    key: &str,
    relative: &str,
) -> Result<PathBuf, EvidenceRefusal> {
    let candidate = bundle_dir.join(relative);
    match candidate.canonicalize() {
        Ok(resolved) if resolved.starts_with(bundle_dir) => Ok(resolved),
        Ok(resolved) => Err(EvidenceRefusal::new(
            format!(
                "escaping: files.{key} {relative} resolves outside the bundle to {}",
                resolved.display()
            ),
            format!("keep every listed file inside {}", bundle_dir.display()),
        )),
        Err(_) => Err(EvidenceRefusal::new(
            format!(
                "incomplete: files.{key} ({}) does not exist",
                candidate.display()
            ),
            producing_command(key, bundle_dir),
        )),
    }
}

/// Passed and failed `Expect` steps recorded in a Playwright trace.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ExpectSummary {
    pub passed: usize,
    pub failed: usize,
}

/// Read `test.trace` from a Playwright trace zip and count final Expect steps.
/// Inner assertions retried by `expect.poll` or `toPass` are children of an
/// outer Expect step; only that outer step's outcome describes the test.
/// A top-level `error` event still counts as a failure.
pub fn trace_expect_summary(trace_zip: &Path) -> Result<ExpectSummary, String> {
    let file =
        std::fs::File::open(trace_zip).map_err(|error| format!("cannot be opened ({error})"))?;
    let mut archive =
        zip::ZipArchive::new(file).map_err(|error| format!("is not a zip ({error})"))?;
    let mut body = String::new();
    archive
        .by_name("test.trace")
        .map_err(|_| {
            // cas-b10d: a library `context.tracing` zip records protocol calls,
            // not the test's outcome, so its assertions cannot be counted.
            "has no test.trace: record it with the Playwright test runner (trace: 'on'); a library context.tracing zip carries no test outcome".to_string()
        })?
        .read_to_string(&mut body)
        .map_err(|error| format!("test.trace is unreadable ({error})"))?;
    Ok(expect_summary_from_events(&body))
}

fn expect_summary_from_events(body: &str) -> ExpectSummary {
    // Playwright writes callId=stepId and parentId=the enclosing test step.
    // Collect the full graph before outcomes because a trace can interleave
    // child and parent `after` events, and a custom poll message can hide the
    // word "poll" in the outer title. An Expect ancestor identifies retries.
    let mut steps = std::collections::HashMap::new();
    for event in body
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
    {
        if event.get("type").and_then(|value| value.as_str()) != Some("before") {
            continue;
        }
        let Some(call) = event.get("callId").and_then(|value| value.as_str()) else {
            continue;
        };
        let title = event
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let expect = event.get("method").and_then(|value| value.as_str()) == Some("expect")
            || title.starts_with("Expect \"");
        let parent = event
            .get("parentId")
            .and_then(|value| value.as_str())
            .map(str::to_string);
        steps.insert(call.to_string(), (parent, expect));
    }
    let mut summary = ExpectSummary::default();
    for event in body
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
    {
        let kind = event
            .get("type")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        let call = event
            .get("callId")
            .and_then(|value| value.as_str())
            .unwrap_or_default();
        match kind {
            "after" if steps.get(call).is_some_and(|(_, expect)| *expect)
                && !expect_has_expect_ancestor(call, &steps) => {
                if event.get("error").is_some_and(|error| !error.is_null()) {
                    summary.failed += 1;
                } else {
                    summary.passed += 1;
                }
            }
            "error" => summary.failed += 1,
            _ => {}
        }
    }
    summary
}

fn expect_has_expect_ancestor(
    call: &str,
    steps: &std::collections::HashMap<String, (Option<String>, bool)>,
) -> bool {
    let mut seen = std::collections::HashSet::new();
    let mut parent = steps.get(call).and_then(|(parent, _)| parent.as_deref());
    while let Some(id) = parent {
        if !seen.insert(id) {
            break;
        }
        let Some((next, is_expect)) = steps.get(id) else {
            break;
        };
        if *is_expect {
            return true;
        }
        parent = next.as_deref();
    }
    false
}

/// Validate `<task>/LEDGER.md` for a demo-only (non-web) delivery: present,
/// non-empty and fresher than the delivered commit. A real-build PASS or a
/// deployed-verification deferral is required; the close handler authenticates
/// deferred owners and records their post-deploy obligations.
pub fn validate_ledger(ctx: &EvidenceContext<'_>) -> Result<PathBuf, EvidenceRefusal> {
    validate_ledger_receipt(ctx).map(|receipt| receipt.0)
}

/// A deployed check deferred to a registered supervisor. This is not a PASS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredDeployedVerification {
    pub row_id: String,
    pub owner: String,
    pub ledger: PathBuf,
}

fn validate_ledger_receipt(
    ctx: &EvidenceContext<'_>,
) -> Result<(PathBuf, Vec<DeferredDeployedVerification>), EvidenceRefusal> {
    let ledger = ctx.task_artifacts_dir.join("LEDGER.md");
    let command = format!(
        "walk the demo_statement and write the evidence ledger to {} (cas-qa-craft references/evidence-ledger.md)",
        ledger.display()
    );
    let body = std::fs::read_to_string(&ledger).map_err(|_| {
        EvidenceRefusal::new(
            format!("missing: no ledger at {}", ledger.display()),
            command.clone(),
        )
    })?;
    if body.trim().is_empty() {
        return Err(EvidenceRefusal::new(
            format!("empty: {} has no rows", ledger.display()),
            command,
        ));
    }
    if let Some(delivered) = committer_time(ctx.repo, ctx.delivered_head)
        && mtime_secs(&ledger).is_some_and(|mtime| mtime < delivered)
    {
        return Err(EvidenceRefusal::new(
            format!(
                "stale: {} predates the delivered commit {}",
                ledger.display(),
                short(ctx.delivered_head)
            ),
            command,
        ));
    }
    let mut has_pass = false;
    let mut deferred = Vec::new();
    for line in body.lines() {
        let cells: Vec<&str> = line
            .trim()
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect();
        // Row grammar: id | cell | expected | observed | verdict | label | evidence | defect.
        has_pass |= cells.get(4) == Some(&"PASS") && cells.get(5) == Some(&"real-build");
        if cells.get(4) != Some(&"DEFERRED") || cells.get(5) != Some(&"deployed-verification") {
            continue;
        }
        let owner = cells
            .get(6)
            .and_then(|cell| cell.strip_prefix("deferred: deployed-verification owner="));
        let Some(owner) = owner.filter(|owner| {
            !owner.is_empty() && !owner.chars().any(|c| c.is_whitespace() || c.is_control())
        }) else {
            return Err(EvidenceRefusal::new(
                "invalid deployed-verification deferral: expected `deferred: deployed-verification owner=<registered supervisor>`",
                command,
            ));
        };
        let row_id = cells[0];
        if row_id.is_empty() {
            return Err(EvidenceRefusal::new(
                "invalid deployed-verification deferral: row id is empty",
                command,
            ));
        }
        deferred.push(DeferredDeployedVerification {
            row_id: row_id.into(),
            owner: owner.into(),
            ledger: ledger.clone(),
        });
    }
    if !has_pass && deferred.is_empty() {
        return Err(EvidenceRefusal::new(
            format!(
                "unproven: {} has no row with verdict PASS and label real-build",
                ledger.display()
            ),
            command,
        ));
    }
    Ok((ledger, deferred))
}

/// First line of a passing cas-cli-craft `scripts/terminal-qa.mjs` report.
pub const TERMINAL_QA_PASS: &str = "terminal-qa: PASS";

/// Validate a terminal-qa receipt under `<task>/terminal-qa/`: some
/// `report.md` whose first line is a PASS receipt and which is at least as
/// fresh as the delivered commit.
pub fn validate_terminal_qa(ctx: &EvidenceContext<'_>) -> Result<PathBuf, EvidenceRefusal> {
    let root = ctx.task_artifacts_dir.join("terminal-qa");
    let command = format!(
        "node <skills-dir>/cas-cli-craft/scripts/terminal-qa.mjs --label <command> --out {}/<command> -- <command> (cas-cli-craft step 7)",
        root.display()
    );
    let delivered = committer_time(ctx.repo, ctx.delivered_head);
    let mut reports = Vec::new();
    let mut stack = vec![root.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.file_name().is_some_and(|name| name == "report.md") {
                reports.push(path);
            }
        }
    }
    if reports.is_empty() {
        return Err(EvidenceRefusal::new(
            format!(
                "missing: the diff changes terminal rendering and {} holds no terminal-qa report.md",
                root.display()
            ),
            command,
        ));
    }
    reports.sort();
    let mut stale = None;
    for report in &reports {
        let first = std::fs::read_to_string(report)
            .unwrap_or_default()
            .lines()
            .next()
            .unwrap_or_default()
            .trim()
            .to_string();
        if !first.starts_with(TERMINAL_QA_PASS) {
            continue;
        }
        if delivered.is_some_and(|time| mtime_secs(report).is_some_and(|mtime| mtime < time)) {
            stale = Some(report.clone());
            continue;
        }
        return Ok(report.clone());
    }
    Err(EvidenceRefusal::new(
        match stale {
            Some(report) => format!(
                "stale: {} passed before the delivered commit {}",
                report.display(),
                short(ctx.delivered_head)
            ),
            None => format!(
                "failing: no report.md under {} starts with `{TERMINAL_QA_PASS}`",
                root.display()
            ),
        },
        command,
    ))
}

/// One skip marker the delivery adds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipMarker {
    pub file: String,
    pub line: usize,
    pub marker: &'static str,
    /// `Some(reason)` when a `cas-allow-skip:` reason accompanies it.
    pub allowed: Option<String>,
}

const SKIP_MARKERS: [&str; 12] = [
    "test.describe.fixme(",
    "test.describe.skip(",
    "test.describe.only(",
    "test.fixme(",
    "test.skip(",
    "test.only(",
    "describe.skip(",
    "describe.only(",
    "it.skip(",
    "it.only(",
    "xdescribe(",
    "xit(",
];

/// Whether a path is a JS/TS test file whose skip markers the gate reads.
pub fn is_js_test_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let file = lower.rsplit('/').next().unwrap_or(&lower);
    let scripted = [".js", ".jsx", ".ts", ".tsx", ".mjs", ".cjs", ".mts", ".cts"]
        .iter()
        .any(|ext| file.ends_with(ext));
    let in_dir =
        |dir: &str| lower.starts_with(&format!("{dir}/")) || lower.contains(&format!("/{dir}/"));
    scripted
        && (file.contains(".spec.")
            || file.contains(".test.")
            || ["e2e", "tests", "test", "__tests__"]
                .iter()
                .any(|dir| in_dir(dir)))
}

fn marker_in(line: &str) -> Option<&'static str> {
    SKIP_MARKERS.iter().copied().find(|marker| {
        line.match_indices(marker).any(|(index, _)| {
            // `xit(` / `it.skip(` must not match inside `exit(` / `unit.skip(`.
            index == 0
                || !line[..index]
                    .chars()
                    .next_back()
                    .is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
        })
    })
}

fn allow_reason(line: &str) -> Option<String> {
    let index = line.find(ALLOW_SKIP)?;
    let reason = line[index + ALLOW_SKIP.len()..]
        .trim()
        .trim_end_matches("*/")
        .trim();
    (!reason.is_empty()).then(|| reason.to_string())
}

/// Skip markers on the added lines of a `git diff -U<n>` body. A marker is
/// allowed when its own line or the line directly above it (added or
/// context) carries `cas-allow-skip: <reason>`.
pub fn added_skip_markers(diff: &str) -> Vec<SkipMarker> {
    let mut markers = Vec::new();
    let mut file: Option<String> = None;
    let mut new_line = 0usize;
    let mut previous: Option<String> = None;
    for raw in diff.lines() {
        if let Some(path) = raw.strip_prefix("+++ ") {
            file = path
                .strip_prefix("b/")
                .map(str::to_string)
                .filter(|_| path != "/dev/null");
            previous = None;
            continue;
        }
        if raw.starts_with("--- ") || raw.starts_with("diff --git") {
            continue;
        }
        if let Some(header) = raw.strip_prefix("@@") {
            new_line = header
                .split_whitespace()
                .find_map(|part| part.strip_prefix('+'))
                .and_then(|range| range.split(',').next())
                .and_then(|start| start.parse().ok())
                .unwrap_or(0);
            previous = None;
            continue;
        }
        let Some(path) = file.as_deref() else {
            continue;
        };
        if let Some(added) = raw.strip_prefix('+') {
            if is_js_test_file(path)
                && let Some(marker) = marker_in(added)
            {
                markers.push(SkipMarker {
                    file: path.to_string(),
                    line: new_line,
                    marker,
                    allowed: allow_reason(added)
                        .or_else(|| previous.as_deref().and_then(allow_reason)),
                });
            }
            previous = Some(added.to_string());
            new_line += 1;
        } else if let Some(context) = raw.strip_prefix(' ') {
            previous = Some(context.to_string());
            new_line += 1;
        } else if raw.starts_with('-') {
            // Removed lines do not advance the new-file counter.
        } else {
            previous = None;
        }
    }
    markers
}

/// `git diff -U1 <from> <to>` restricted to JS/TS test files; `None` when git fails.
pub fn delivery_test_diff(repo: &Path, from: &str, to: &str, paths: &[String]) -> Option<String> {
    let tests: Vec<&String> = paths.iter().filter(|path| is_js_test_file(path)).collect();
    if tests.is_empty() {
        return Some(String::new());
    }
    let mut args: Vec<String> = vec![
        "diff".into(),
        "-U1".into(),
        "--no-color".into(),
        "--no-ext-diff".into(),
        from.into(),
        to.into(),
        "--".into(),
    ];
    args.extend(tests.into_iter().cloned());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = Command::new("git")
        .args(&refs)
        .current_dir(repo)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The commit range a delivery contributes: `merge-base(target, head)..head`
/// before it merges; after it merges, the first merge on the ancestry path
/// from `head` to `target` (`M^1..M`). `None` when neither can be computed
/// (for example a fast-forward merge, which leaves no merge commit).
pub fn delivery_range(repo: &Path, head: &str, target: &str) -> Option<(String, String)> {
    let merged = git(repo, &["merge-base", "--is-ancestor", head, target]).is_some();
    if merged {
        let merges = git(
            repo,
            &[
                "rev-list",
                "--ancestry-path",
                "--merges",
                "--reverse",
                &format!("{head}..{target}"),
            ],
        )?;
        let merge = merges.lines().next()?.trim().to_string();
        Some((format!("{merge}^1"), merge))
    } else {
        Some((git(repo, &["merge-base", target, head])?, head.to_string()))
    }
}

/// Reviewable paths changed over a range; deletions and whitespace-only
/// changes cannot require a UI evidence bundle.
pub fn range_paths(repo: &Path, from: &str, to: &str) -> Option<Vec<String>> {
    Some(
        git(repo, &["diff", "-w", "--diff-filter=ACMRT", "--name-only", from, to])?
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(ToOwned::to_owned)
            .collect(),
    )
}

/// What the implementer must show for a user-facing delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EvidenceTier {
    /// Not user-facing: no evidence required.
    None,
    /// Web surface touched: the cas-c3b8 bundle.
    Bundle,
    /// demo_statement only, no web surface in the diff: the evidence ledger
    /// with a real-build PASS row, plus a cas-cli-craft terminal-qa PASS
    /// receipt when the diff changes command output. Interactive TUI/PTY
    /// surfaces can use the ledger alone, as classified by the close gate.
    Ledger { terminal_qa: bool },
}

/// Result of the close gate when it does not reject.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatePass {
    /// Decision-note lines to append at close (allowed skips, receipts).
    pub notes: Vec<String>,
    /// The close handler must authenticate each owner before accepting these.
    pub deferred_deployed: Vec<DeferredDeployedVerification>,
}

/// Run the evidence and skip-marker checks and render the rejection text.
/// `reasons` explain why the delivery is user-facing (empty for `None`).
pub fn run_close_gate(
    ctx: &EvidenceContext<'_>,
    tier: EvidenceTier,
    reasons: &[String],
    skip_markers: &[SkipMarker],
) -> Result<GatePass, String> {
    run_close_gate_with_write_dir(ctx, tier, reasons, skip_markers, ctx.task_artifacts_dir)
}

/// Read historical evidence from `ctx`, but direct replacement evidence to
/// the current project namespace. The problem retains the historical path.
pub fn run_close_gate_with_write_dir(
    ctx: &EvidenceContext<'_>,
    tier: EvidenceTier,
    reasons: &[String],
    skip_markers: &[SkipMarker],
    write_dir: &Path,
) -> Result<GatePass, String> {
    let mut pass = GatePass::default();
    let blocked: Vec<&SkipMarker> = skip_markers
        .iter()
        .filter(|marker| marker.allowed.is_none())
        .collect();
    if !blocked.is_empty() {
        let list = blocked
            .iter()
            .map(|marker| {
                format!(
                    "{}:{} `{}`",
                    marker.file,
                    marker.line,
                    marker.marker.trim_end_matches('(')
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        return Err(format!(
            "TASK CLOSE REJECTED: {task} adds test skip/focus markers without a stated reason: {list}. \
             A fixme or skip turns a failing check into a green run, and `.only` silently drops every other test. \
             Remove the marker and fix the test or the product. If the skip is deliberate (for example a real browser limitation), \
             put `// {ALLOW_SKIP} <reason>` on the marker's line or the line above it, then retry close.",
            task = ctx.task_id,
        ));
    }
    for marker in skip_markers {
        if let Some(reason) = marker.allowed.as_deref() {
            pass.notes.push(format!(
                "Allowed skip marker {}:{} `{}` — {reason}",
                marker.file,
                marker.line,
                marker.marker.trim_end_matches('(')
            ));
        }
    }
    let why = reasons.join("; ");
    let reject = |refusal: EvidenceRefusal, what: &str| {
        let mut command = refusal.command.replace(
            ctx.task_artifacts_dir.to_string_lossy().as_ref(),
            write_dir.to_string_lossy().as_ref(),
        );
        // Generic repair hints (for example malformed manifests) do not name
        // a path. Historical evidence is read-only to factory workers, so
        // make the writable replacement and its citation explicit as well.
        if ctx.task_artifacts_dir != write_dir
            && !command.contains(write_dir.to_string_lossy().as_ref())
        {
            let write_ctx = EvidenceContext { task_artifacts_dir: write_dir, ..*ctx };
            command.push_str(&format!("; {}", cite_command(&write_ctx)));
        }
        format!(
            "TASK CLOSE REJECTED: {task} is user-facing ({why}) and its {what} is {problem}. \
             Next: {command}. Then retry close. Contract: {CONTRACT_REFERENCE}.",
            task = ctx.task_id,
            problem = refusal.problem,
            command = command,
        )
    };
    if let Some(selection) = journeys::check_close_journeys(ctx, reasons)
        .map_err(|refusal| reject(refusal, "affected journey receipt"))?
    {
        pass.notes.push(selection);
    }
    match tier {
        EvidenceTier::None => {}
        EvidenceTier::Bundle => {
            let receipt =
                validate_bundle(ctx).map_err(|refusal| reject(refusal, "QA evidence bundle"))?;
            pass.notes.push(format!(
                "QA evidence bundle accepted: {} (head {}, {} passing Expect step(s)).",
                receipt.manifest.display(),
                short(&receipt.head_sha),
                receipt.passed_expects
            ));
        }
        EvidenceTier::Ledger { terminal_qa } => {
            let (ledger, deferred) = validate_ledger_receipt(ctx)
                .map_err(|refusal| reject(refusal, "QA evidence ledger"))?;
            pass.deferred_deployed = deferred;
            pass.notes.push(format!(
                "QA evidence ledger accepted: {}.",
                ledger.display()
            ));
            if terminal_qa {
                let report = validate_terminal_qa(ctx)
                    .map_err(|refusal| reject(refusal, "terminal-qa receipt"))?;
                pass.notes.push(format!(
                    "terminal-qa receipt accepted: {}.",
                    report.display()
                ));
            }
        }
    }
    Ok(pass)
}

#[cfg(test)]
#[path = "qa_evidence_tests.rs"]
mod tests;
