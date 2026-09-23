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

/// Note token that cites a bundle manifest: `qa-bundle: <abs>/bundle.json`.
pub const BUNDLE_CITATION: &str = "qa-bundle:";
/// Same-line (or preceding-line) escape for a deliberate skip marker.
pub const ALLOW_SKIP: &str = "cas-allow-skip:";
/// Where the contract's worked example and producing commands live.
pub const CONTRACT_REFERENCE: &str = "cas-qa-craft references/evidence-bundle.md";

const RUBRIC: [&str; 5] = ["distinctiveness", "fit", "hierarchy", "craft", "accessibility"];
const FLOOR_DIMENSIONS: [&str; 3] = ["distinctiveness", "fit", "hierarchy"];
const FLOOR: u8 = 4;
const A11Y_MODES: [&str; 3] = ["forced-colors", "reduced-motion", "contrast-more"];
const POLISH_RENDERS: [&str; 4] = ["-light-desktop.png", "-light-phone.png", "-dark-desktop.png", "-dark-phone.png"];

/// Everything the validator needs, resolved by the caller.
#[derive(Debug, Clone)]
pub struct EvidenceContext<'a> {
    pub task_id: &'a str,
    /// `<artifacts_root>/<task-id>`.
    pub task_artifacts_dir: &'a Path,
    /// Repository holding the delivered head (for committer time/ancestry).
    pub repo: &'a Path,
    /// Full SHA of the delivered commit.
    pub delivered_head: &'a str,
    /// The task's notes, where the `qa-bundle:` citation lives.
    pub notes: &'a str,
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
    created_at: String,
    #[serde(default)]
    visual_change: bool,
    #[serde(default)]
    visual_qa_status: String,
    files: ManifestFiles,
    #[serde(default)]
    critique_score: std::collections::BTreeMap<String, i64>,
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

fn expand_home(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => dirs::home_dir().map(|home| home.join(rest)).unwrap_or_else(|| PathBuf::from(path)),
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
    let output = Command::new("git").args(args).current_dir(repo).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Committer time (unix seconds) of a commit.
pub fn committer_time(repo: &Path, commit: &str) -> Option<i64> {
    git(repo, &["show", "-s", "--format=%ct", commit])?.parse().ok()
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
    let cited = cited_bundle_path(ctx.notes)
        .ok_or_else(|| EvidenceRefusal::new(format!("not cited (no `{BUNDLE_CITATION}` note)"), cite_command(ctx)))?;
    let manifest_path = resolve_inside_task_dir(ctx, &cited)?;
    let bundle_dir = manifest_path
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| EvidenceRefusal::new(format!("cited at {cited}, which has no directory"), cite_command(ctx)))?;

    let raw = std::fs::read_to_string(&manifest_path).map_err(|error| {
        EvidenceRefusal::new(format!("unreadable at {} ({error})", manifest_path.display()), cite_command(ctx))
    })?;
    let manifest: Manifest = serde_json::from_str(&raw).map_err(|error| {
        EvidenceRefusal::new(
            format!("malformed: {} does not parse as a v1 manifest ({error})", manifest_path.display()),
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
            format!("for task {} (bundle.json task_id), not {}", manifest.task_id, ctx.task_id),
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
        listed.push((key.to_string(), resolve_bundle_file(&bundle_dir, key, relative)?));
    }
    if files.cells.is_empty() {
        return Err(missing_key("cells"));
    }
    for (index, relative) in files.cells.iter().enumerate() {
        listed.push((format!("cells[{index}]"), resolve_bundle_file(&bundle_dir, "cells", relative)?));
    }
    for suffix in POLISH_RENDERS {
        if !files.polish_screenshots.iter().any(|path| path.ends_with(suffix)) {
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
        listed.push((format!("a11y[{index}]"), resolve_bundle_file(&bundle_dir, "a11y", relative)?));
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
                    format!("incomplete: files.{key} ({}) does not exist", path.display()),
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
            format!("unbound: head_sha {:?} is not a full 40-hex commit", manifest.head_sha),
            rerun,
        ));
    }
    let delivered_time = committer_time(ctx.repo, ctx.delivered_head).ok_or_else(|| {
        EvidenceRefusal::new(
            format!("uncheckable: the delivered head {} is not readable in {}", short(ctx.delivered_head), ctx.repo.display()),
            "push the delivery commit, then retry close".to_string(),
        )
    })?;
    let head_covers_delivery = manifest.head_sha == ctx.delivered_head
        || git(ctx.repo, &["merge-base", "--is-ancestor", ctx.delivered_head, &manifest.head_sha]).is_some();
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
    let created = chrono::DateTime::parse_from_rfc3339(manifest.created_at.trim())
        .map(|time| time.timestamp())
        .map_err(|_| {
            EvidenceRefusal::new(
                format!("unbound: created_at {:?} is not RFC 3339", manifest.created_at),
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
    let path_of = |key: &str| listed.iter().find(|(name, _)| name == key).map(|(_, path)| path.clone());
    let trace_path = path_of("trace").unwrap_or_default();
    let summary = trace_expect_summary(&trace_path).map_err(|error| {
        EvidenceRefusal::new(format!("unusable: files.trace {error}"), producing_command("trace", &bundle_dir))
    })?;
    if summary.failed > 0 {
        return Err(EvidenceRefusal::new(
            format!(
                "a failing run: files.trace records {} failed Expect step(s) — evidence must come from a passing run",
                summary.failed
            ),
            format!("fix the failure, then {}", producing_command("trace", &bundle_dir)),
        ));
    }
    if summary.passed == 0 {
        return Err(EvidenceRefusal::new(
            "assertion-free: files.trace contains no passing Expect step",
            producing_command("trace", &bundle_dir),
        ));
    }
    let actions = std::fs::read_to_string(path_of("trace_actions").unwrap_or_default()).unwrap_or_default();
    if !actions.lines().any(|line| line.contains("Expect \"")) {
        return Err(EvidenceRefusal::new(
            "incomplete: files.trace_actions lists no `Expect \"` step",
            producing_command("trace_actions", &bundle_dir),
        ));
    }

    // 5. Polish run passed.
    if manifest.visual_qa_status != "pass" {
        return Err(EvidenceRefusal::new(
            format!(
                "missing polish proof: visual_qa_status is {:?} (only \"pass\" closes without a supervisor override)",
                manifest.visual_qa_status
            ),
            producing_command("visual_qa_stdout", &bundle_dir),
        ));
    }
    let visual_qa = std::fs::read_to_string(path_of("visual_qa").unwrap_or_default()).unwrap_or_default();
    if !visual_qa.lines().next().is_some_and(|line| line.contains("PASS")) {
        return Err(EvidenceRefusal::new(
            "missing polish proof: files.visual_qa does not start with `# Visual QA — PASS`",
            producing_command("visual_qa_stdout", &bundle_dir),
        ));
    }
    let stdout = std::fs::read_to_string(path_of("visual_qa_stdout").unwrap_or_default()).unwrap_or_default();
    if stdout.lines().rev().find(|line| !line.trim().is_empty()).map(str::trim) != Some("PASS") {
        return Err(EvidenceRefusal::new(
            "missing polish proof: files.visual_qa_stdout does not end with `PASS`",
            producing_command("visual_qa_stdout", &bundle_dir),
        ));
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
            let floor = if FLOOR_DIMENSIONS.contains(dimension) { FLOOR as i64 } else { 1 };
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
        "trace" | "receipt" | "aria_yaml" | "aria_json" | "cells" | "a11y" => format!(
            "run the cas-qa-craft evidence spec with trace snapshots {{ dom, aria, screen }} and write {key} into {dir} ({CONTRACT_REFERENCE})"
        ),
        "trace_actions" => format!(
            "cd {dir} && npx playwright trace open trace.zip && npx playwright trace actions > trace-actions.txt; npx playwright trace close"
        ),
        "polish_screenshots" | "visual_qa" | "visual_qa_json" | "visual_qa_stdout" => format!(
            "node scripts/visual-qa.mjs --strict --artifact-dir {dir}/visual-qa <url> > {dir}/visual-qa.stdout 2>&1"
        ),
        "critique" => format!("score the surface with the cas-ui-craft rubric into {dir}/critique.md and bundle.json critique_score"),
        _ => format!("produce `{key}` per {CONTRACT_REFERENCE}"),
    }
}

fn resolve_inside_task_dir(ctx: &EvidenceContext<'_>, cited: &str) -> Result<PathBuf, EvidenceRefusal> {
    let candidate = expand_home(cited);
    let resolved = candidate.canonicalize().map_err(|_| {
        EvidenceRefusal::new(format!("missing: the cited {cited} does not exist"), cite_command(ctx))
    })?;
    let root = ctx.task_artifacts_dir.canonicalize().map_err(|_| {
        EvidenceRefusal::new(
            format!("missing: the task artifacts dir {} does not exist", ctx.task_artifacts_dir.display()),
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

fn resolve_bundle_file(bundle_dir: &Path, key: &str, relative: &str) -> Result<PathBuf, EvidenceRefusal> {
    let candidate = bundle_dir.join(relative);
    match candidate.canonicalize() {
        Ok(resolved) if resolved.starts_with(bundle_dir) => Ok(resolved),
        Ok(resolved) => Err(EvidenceRefusal::new(
            format!("escaping: files.{key} {relative} resolves outside the bundle to {}", resolved.display()),
            format!("keep every listed file inside {}", bundle_dir.display()),
        )),
        Err(_) => Err(EvidenceRefusal::new(
            format!("incomplete: files.{key} ({}) does not exist", candidate.display()),
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

/// Read `test.trace` from a Playwright trace zip and count Expect steps.
/// A step fails when its `after` event carries an `error`; a top-level
/// `error` event also counts as a failure.
pub fn trace_expect_summary(trace_zip: &Path) -> Result<ExpectSummary, String> {
    let file = std::fs::File::open(trace_zip).map_err(|error| format!("cannot be opened ({error})"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|error| format!("is not a zip ({error})"))?;
    let mut body = String::new();
    archive
        .by_name("test.trace")
        .map_err(|_| "has no test.trace (record it with the Playwright test runner)".to_string())?
        .read_to_string(&mut body)
        .map_err(|error| format!("test.trace is unreadable ({error})"))?;
    Ok(expect_summary_from_events(&body))
}

fn expect_summary_from_events(body: &str) -> ExpectSummary {
    let mut expects = std::collections::HashSet::new();
    let mut summary = ExpectSummary::default();
    for line in body.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = event.get("type").and_then(|value| value.as_str()).unwrap_or_default();
        let call = event.get("callId").and_then(|value| value.as_str()).unwrap_or_default();
        match kind {
            "before" => {
                let title = event.get("title").and_then(|value| value.as_str()).unwrap_or_default();
                let method = event.get("method").and_then(|value| value.as_str()).unwrap_or_default();
                if method == "expect" || title.starts_with("Expect \"") {
                    expects.insert(call.to_string());
                }
            }
            "after" if expects.contains(call) => {
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

/// Validate `<task>/LEDGER.md` for a demo-only (non-web) delivery: present,
/// non-empty, fresher than the delivered commit, with at least one PASS row.
pub fn validate_ledger(ctx: &EvidenceContext<'_>) -> Result<PathBuf, EvidenceRefusal> {
    let ledger = ctx.task_artifacts_dir.join("LEDGER.md");
    let command = format!(
        "walk the demo_statement and write the evidence ledger to {} (cas-qa-craft references/evidence-ledger.md)",
        ledger.display()
    );
    let body = std::fs::read_to_string(&ledger)
        .map_err(|_| EvidenceRefusal::new(format!("missing: no ledger at {}", ledger.display()), command.clone()))?;
    if body.trim().is_empty() {
        return Err(EvidenceRefusal::new(format!("empty: {} has no rows", ledger.display()), command));
    }
    if let Some(delivered) = committer_time(ctx.repo, ctx.delivered_head)
        && mtime_secs(&ledger).is_some_and(|mtime| mtime < delivered)
    {
        return Err(EvidenceRefusal::new(
            format!("stale: {} predates the delivered commit {}", ledger.display(), short(ctx.delivered_head)),
            command,
        ));
    }
    let has_pass = body.lines().any(|line| {
        let cells: Vec<&str> = line.trim().trim_matches('|').split('|').map(str::trim).collect();
        line.trim_start().starts_with('|') && cells.get(4) == Some(&"PASS")
    });
    if !has_pass {
        return Err(EvidenceRefusal::new(
            format!("unproven: {} has no row with verdict PASS", ledger.display()),
            command,
        ));
    }
    Ok(ledger)
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
    let in_dir = |dir: &str| lower.starts_with(&format!("{dir}/")) || lower.contains(&format!("/{dir}/"));
    scripted && (file.contains(".spec.") || file.contains(".test.") || ["e2e", "tests", "test", "__tests__"].iter().any(|dir| in_dir(dir)))
}

fn marker_in(line: &str) -> Option<&'static str> {
    SKIP_MARKERS.iter().copied().find(|marker| {
        line.match_indices(marker).any(|(index, _)| {
            // `xit(` / `it.skip(` must not match inside `exit(` / `unit.skip(`.
            index == 0 || !line[..index].chars().next_back().is_some_and(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '$')
        })
    })
}

fn allow_reason(line: &str) -> Option<String> {
    let index = line.find(ALLOW_SKIP)?;
    let reason = line[index + ALLOW_SKIP.len()..].trim().trim_end_matches("*/").trim();
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
            file = path.strip_prefix("b/").map(str::to_string).filter(|_| path != "/dev/null");
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
        let Some(path) = file.as_deref() else { continue };
        if let Some(added) = raw.strip_prefix('+') {
            if is_js_test_file(path)
                && let Some(marker) = marker_in(added)
            {
                markers.push(SkipMarker {
                    file: path.to_string(),
                    line: new_line,
                    marker,
                    allowed: allow_reason(added).or_else(|| previous.as_deref().and_then(allow_reason)),
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
    let mut args: Vec<String> = vec!["diff".into(), "-U1".into(), "--no-color".into(), "--no-ext-diff".into(), from.into(), to.into(), "--".into()];
    args.extend(tests.into_iter().cloned());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let output = Command::new("git").args(&refs).current_dir(repo).output().ok()?;
    output.status.success().then(|| String::from_utf8_lossy(&output.stdout).into_owned())
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
            &["rev-list", "--ancestry-path", "--merges", "--reverse", &format!("{head}..{target}")],
        )?;
        let merge = merges.lines().next()?.trim().to_string();
        Some((format!("{merge}^1"), merge))
    } else {
        Some((git(repo, &["merge-base", target, head])?, head.to_string()))
    }
}

/// Paths changed over a range.
pub fn range_paths(repo: &Path, from: &str, to: &str) -> Option<Vec<String>> {
    Some(
        git(repo, &["diff", "--name-only", from, to])?
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
    /// demo_statement only, no web surface in the diff: the evidence ledger.
    Ledger,
}

/// Result of the close gate when it does not reject.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GatePass {
    /// Decision-note lines to append at close (allowed skips, receipts).
    pub notes: Vec<String>,
}

/// Run the evidence and skip-marker checks and render the rejection text.
/// `reasons` explain why the delivery is user-facing (empty for `None`).
pub fn run_close_gate(
    ctx: &EvidenceContext<'_>,
    tier: EvidenceTier,
    reasons: &[String],
    skip_markers: &[SkipMarker],
) -> Result<GatePass, String> {
    let mut pass = GatePass::default();
    let blocked: Vec<&SkipMarker> = skip_markers.iter().filter(|marker| marker.allowed.is_none()).collect();
    if !blocked.is_empty() {
        let list = blocked
            .iter()
            .map(|marker| format!("{}:{} `{}`", marker.file, marker.line, marker.marker.trim_end_matches('(')))
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
        format!(
            "TASK CLOSE REJECTED: {task} is user-facing ({why}) and its {what} is {problem}. \
             Next: {command}. Then retry close. Contract: {CONTRACT_REFERENCE}.",
            task = ctx.task_id,
            problem = refusal.problem,
            command = refusal.command,
        )
    };
    match tier {
        EvidenceTier::None => {}
        EvidenceTier::Bundle => {
            let receipt = validate_bundle(ctx).map_err(|refusal| reject(refusal, "QA evidence bundle"))?;
            pass.notes.push(format!(
                "QA evidence bundle accepted: {} (head {}, {} passing Expect step(s)).",
                receipt.manifest.display(),
                short(&receipt.head_sha),
                receipt.passed_expects
            ));
        }
        EvidenceTier::Ledger => {
            let ledger = validate_ledger(ctx).map_err(|refusal| reject(refusal, "QA evidence ledger"))?;
            pass.notes.push(format!("QA evidence ledger accepted: {}.", ledger.display()));
        }
    }
    Ok(pass)
}

#[cfg(test)]
#[path = "qa_evidence_tests.rs"]
mod tests;
