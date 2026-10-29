//! Assemble and render a release report from the repository's release evidence.
//!
//! The report source is deliberately assembled in Rust while the visual
//! surface remains owned by the portable `cas-release-report` builtin.  This
//! keeps source acquisition testable and lets projects replace the renderer
//! through their installed skill without embedding project-specific HTML in
//! the CLI.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use anyhow::{Context, bail};
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, Utc};
use clap::Args;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::bounded_process::{BoundedCommandError, Deadline, run_command};
use crate::deliverable::Audience;
use crate::deliverable::asset::{Asset, Workspace};
use crate::deliverable::finalize::{self, Budget, FinalizeRequest, Paper};
use crate::cli::Cli;
use crate::config::Config;
use crate::store::find_cas_root_from;
use crate::ui::components::{Formatter, Verdict, ascii_fallback};
use crate::ui::theme::ActiveTheme;

const GH_TIMEOUT: Duration = Duration::from_secs(20);
const RENDER_TIMEOUT: Duration = Duration::from_secs(45);
const RENDER_SCRIPT: &str = "skills/cas-release-report/scripts/render.py";
const RENDER_TEMPLATE: &str = "skills/cas-release-report/references/template.html";
/// Issues are closed by hand a few minutes after the release PR merges
/// (6–7 minutes in the October 2026 releases). A closure up to this long after
/// the merge belongs to the release.
const CLOSURE_WINDOW_MINUTES: i64 = 15;
const CLOSURE_POLL: Duration = Duration::from_secs(30);

/// The pause between issue-state reads; `CAS_RELEASE_REPORT_POLL_SECS`
/// shortens it for fixtures.
fn closure_poll() -> Duration {
    std::env::var("CAS_RELEASE_REPORT_POLL_SECS")
        .ok()
        .and_then(|value| value.parse().ok())
        .map(Duration::from_secs)
        .unwrap_or(CLOSURE_POLL)
}

fn closure_window() -> chrono::Duration {
    chrono::Duration::minutes(CLOSURE_WINDOW_MINUTES)
}

/// Generate a release-report Markdown source and its standalone render.
#[derive(Args, Debug, Clone)]
pub struct ReleaseReportArgs {
    /// Release version or tag (for example, 3.20.0 or v3.20.0)
    pub version: String,

    /// Directory receiving <version>.md, <version>.html, and (with --pdf) <version>.pdf
    #[arg(long, default_value = "docs/release-reports")]
    pub out: PathBuf,

    /// Render a PDF with Playwright after rendering the HTML
    #[arg(long)]
    pub pdf: bool,

    /// Re-fetch sources and replace the Markdown source, including a manual source
    #[arg(long)]
    pub refresh_sources: bool,

    /// A standalone `release-latency.receipt` from
    /// `scripts/release-latency-receipt.sh <tag> --run-dir <dir>`; its run
    /// directory is read alongside `<[factory] artifacts_root>/release`
    #[arg(long, value_name = "PATH")]
    pub latency_receipt: Option<PathBuf>,

    /// Who reads the report: customer needs the project's DESIGN.md and
    /// refuses internal vocabulary
    #[arg(long, value_enum, default_value_t = Audience::Internal)]
    pub audience: Audience,
}

#[derive(Debug, Clone, Serialize)]
struct ReportResult {
    version: String,
    tag: String,
    project: String,
    source_path: String,
    html_path: String,
    pdf_path: Option<String>,
    source_written: bool,
    html_written: bool,
    pdf_written: bool,
    issue_count: usize,
    asset_count: usize,
    change_count: usize,
    change_areas: Vec<AreaCount>,
    warnings: Vec<String>,
    retrieved_at: String,
    github_repo: Option<String>,
    release_published_at: Option<String>,
    green_to_published_seconds: Option<i64>,
    /// Which brand source `cas deliverable brand` resolved (design-md-css, …).
    brand_source: String,
    /// `path@revision` of the DESIGN.md the report wears.
    design: Option<String>,
}

#[derive(Debug, Clone)]
struct ChangelogSection {
    heading: String,
    date: Option<String>,
    body: String,
    entries: Vec<ChangelogEntry>,
}

#[derive(Debug, Clone)]
struct ChangelogEntry {
    category: String,
    text: String,
}

#[derive(Debug, Clone, Serialize)]
struct GithubIssue {
    number: u64,
    title: String,
    url: String,
    state: String,
    closed_at: Option<String>,
    labels: Vec<String>,
    body: Option<String>,
    theme: String,
}

#[derive(Debug, Clone, Serialize)]
struct ReleaseAsset {
    name: String,
    size: Option<u64>,
    url: Option<String>,
    digest: Option<String>,
    content_type: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ReleaseMetadata {
    name: Option<String>,
    url: Option<String>,
    body: Option<String>,
    published_at: Option<String>,
    is_draft: Option<bool>,
    assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, Default)]
struct ReleaseEvidence {
    interventions: Option<u64>,
    tag_published_at: Option<String>,
    publication_latency_seconds: Option<i64>,
    publication_start_kind: Option<String>,
    publication_budget_seconds: Option<i64>,
    green_at: Option<String>,
    green_to_published_seconds: Option<i64>,
    main_to_published_seconds: Option<i64>,
    receipt_reference: Option<ReceiptReference>,
    /// cas-a629: the whole wall clock and what cost it.
    request_to_published_seconds: Option<i64>,
    request_source: Option<String>,
    cut_to_published_seconds: Option<i64>,
    blocker_count: Option<u64>,
    /// (stage, seconds, resolved); empty with `blocker_count` set means the
    /// run predates priced blockers.
    blocker_costs: Vec<(String, i64, bool)>,
    /// cas-9734: rescues that repeated an already-learned lesson.
    lesson_recurrence: Option<String>,
    /// cas-2bef: each completed train stage, its versioned budget, the stages
    /// over budget (None when the receipt predates budgets), and earlier
    /// releases' stage times, oldest first.
    stage_seconds: Vec<(String, i64)>,
    stage_budgets: Vec<(String, i64)>,
    budget_breaches: Option<Vec<String>>,
    stage_trend: Vec<(String, Vec<(String, i64)>)>,
    /// (tasks, median seconds, worst task, worst seconds) from park to main.
    commit_to_main: Option<(u64, i64, String, i64)>,
    /// cas-692a: a latency receipt was read, so its absent fields are its own.
    latency_receipt_read: bool,
}

#[derive(Debug, Clone)]
enum ReceiptReference {
    /// A receipt under the artifacts root: how that root is named to the
    /// reader, and the receipt's path relative to it.
    ArtifactsRootRelative { root_label: String, path: String },
    StandaloneInput,
}

/// The sources line for the release receipt.
fn receipt_reference_line(reference: Option<&ReceiptReference>) -> String {
    match reference {
        Some(ReceiptReference::ArtifactsRootRelative { root_label, path }) => {
            format!("Release receipt (relative to `{root_label}`): `{path}`.\n\n")
        }
        Some(ReceiptReference::StandaloneInput) => {
            "Standalone release receipt supplied; its parsed measurements are shown below.\n\n"
                .to_string()
        }
        None => "Release receipt: unavailable.\n\n".to_string(),
    }
}

impl ReleaseEvidence {
    fn record_latency(&mut self, values: &HashMap<String, String>) {
        self.latency_receipt_read = true;
        let seconds = |key: &str| {
            values
                .get(key)
                .and_then(|value| value.parse::<i64>().ok())
                .filter(|seconds| *seconds >= 0)
        };
        self.request_to_published_seconds = seconds("REQUEST_TO_PUBLISHED_SECS");
        self.cut_to_published_seconds = seconds("CUT_TO_PUBLISHED_SECS");
        self.request_source = values
            .get("REQUEST_SOURCE")
            .filter(|source| matches!(source.as_str(), "recorded" | "cut-start"))
            .cloned();
        self.blocker_count = values
            .get("BLOCKER_COUNT")
            .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|value| value.parse().ok());
        self.blocker_costs = values
            .get("BLOCKER_COSTS")
            .map(|costs| parse_blocker_costs(costs))
            .unwrap_or_default();
        self.stage_seconds = values
            .get("STAGE_SECS")
            .map(|pairs| parse_stage_pairs(pairs))
            .unwrap_or_default();
        self.stage_budgets = values
            .get("STAGE_BUDGETS")
            .map(|pairs| parse_stage_pairs(pairs))
            .unwrap_or_default();
        self.budget_breaches = values
            .get("BUDGET_BREACHES")
            .filter(|value| value.as_str() != "unavailable")
            .map(|value| {
                value
                    .split(',')
                    .filter_map(|item| item.split_once(':').map(|(stage, _)| stage.to_string()))
                    .collect()
            });
        self.commit_to_main = values.get("COMMIT_TO_MAIN").and_then(|value| {
            let fields: HashMap<&str, &str> = value
                .split(',')
                .filter_map(|item| item.split_once(':'))
                .collect();
            let (task, worst) = fields.get("worst")?.split_once(':')?;
            Some((
                fields.get("tasks")?.parse().ok()?,
                fields.get("median")?.parse().ok()?,
                task.to_string(),
                worst.parse().ok()?,
            ))
        });
        self.interventions = values
            .get("INTERVENTIONS")
            .filter(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_digit()))
            .and_then(|value| value.parse().ok());
        self.publication_latency_seconds = values
            .get("PUBLISH_LATENCY_SECONDS")
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|seconds| *seconds >= 0);
        self.publication_start_kind = values.get("PUBLISH_START_KIND").cloned();
        self.publication_budget_seconds = values
            .get("BUDGET_SECONDS")
            .and_then(|value| value.parse::<i64>().ok())
            .filter(|budget| *budget >= 0)
            .filter(|budget| {
                self.publication_latency_seconds.is_some_and(|seconds| {
                    values.get("WITHIN_BUDGET").map(String::as_str)
                        == Some(if seconds <= *budget { "true" } else { "false" })
                })
            });
    }

    fn request_timing(&self) -> String {
        match (
            self.request_to_published_seconds,
            self.request_source.as_deref(),
        ) {
            (Some(seconds), Some("cut-start")) => {
                format!(
                    "{} — from the first cut; no request time recorded",
                    format_duration(seconds)
                )
            }
            (Some(seconds), _) => format_duration(seconds),
            (None, _) => "unavailable".to_string(),
        }
    }

    fn blocker_summary(&self) -> String {
        let Some(count) = self.blocker_count else {
            return "unavailable".to_string();
        };
        if count == 0 {
            return "0".to_string();
        }
        if self.blocker_costs.is_empty() {
            return format!("{count} — time cost unavailable");
        }
        let total: i64 = self
            .blocker_costs
            .iter()
            .map(|(_, seconds, _)| seconds)
            .sum();
        let each = self
            .blocker_costs
            .iter()
            .map(|(stage, seconds, resolved)| {
                format!(
                    "{stage} {}{}",
                    format_duration(*seconds),
                    if *resolved {
                        ""
                    } else {
                        " (unresolved at publication)"
                    }
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!("{count} — {} blocked: {each}", format_duration(total))
    }

    fn publication_timing(&self) -> String {
        let Some(seconds) = self.publication_latency_seconds else {
            return "unavailable".to_string();
        };
        let timing = format_duration(seconds);
        match self.publication_budget_seconds {
            Some(budget) => format!(
                "{timing} — {} ({} budget)",
                if seconds <= budget {
                    "within budget"
                } else {
                    "over budget"
                },
                format_duration(budget)
            ),
            None => format!("{timing} — budget unavailable"),
        }
    }

    fn publication_endpoint(&self) -> Option<(&'static str, &'static str)> {
        match self.publication_start_kind.as_deref() {
            Some("main-landed") => Some((
                "Main to published",
                "Release PR merge time to publication when merge-queue green evidence is unavailable; measured budget result",
            )),
            Some("gate-green") => None,
            _ => Some((
                "Tag to published",
                "First tag workflow to publication; measured budget result",
            )),
        }
    }

    fn has_reportable_publication_timing(&self) -> bool {
        self.green_to_published_seconds.is_some() || self.main_to_published_seconds.is_some()
    }
}

/// `stage:secs,…` from the latency receipt's STAGE_SECS or STAGE_BUDGETS.
fn parse_stage_pairs(pairs: &str) -> Vec<(String, i64)> {
    pairs
        .split(',')
        .filter_map(|item| {
            let (stage, seconds) = item.trim().rsplit_once(':')?;
            let seconds = seconds
                .parse::<i64>()
                .ok()
                .filter(|seconds| *seconds >= 0)?;
            (!stage.is_empty()).then(|| (stage.to_string(), seconds))
        })
        .collect()
}

/// The report's latency section (cas-2bef): each stage against its versioned
/// budget with the earlier releases' times, FAILED naming every overrun.
fn pipeline_latency_section(evidence: &ReleaseEvidence) -> String {
    let mut output = String::from("### Pipeline latency\n\n");
    if evidence.stage_seconds.is_empty() {
        output.push_str(if evidence.latency_receipt_read {
            "Stage timings are unavailable: the latency receipt records no STAGE_SECS.\n\n"
        } else {
            "Stage timings are unavailable: no latency receipt was found for this release.\n\n"
        });
        return output;
    }
    let budget = |stage: &str| {
        evidence
            .stage_budgets
            .iter()
            .find(|(name, _)| name == stage)
            .map(|(_, seconds)| *seconds)
    };
    let over: Vec<&(String, i64)> = evidence
        .stage_seconds
        .iter()
        .filter(|(stage, seconds)| budget(stage).is_some_and(|limit| *seconds > limit))
        .collect();
    match &evidence.budget_breaches {
        None => output.push_str(
            "Budgets are unavailable: this receipt predates scripts/release-stage-budgets.json.\n\n",
        ),
        Some(_) if over.is_empty() => output.push_str(
            "Within budget: every measured stage met its budget in scripts/release-stage-budgets.json.\n\n",
        ),
        Some(_) => output.push_str(&format!(
            "FAILED: {} over budget. {}. Post-publication files a P0 task for each over-budget release.\n\n",
            over.len(),
            over.iter()
                .map(|(stage, seconds)| format!(
                    "{stage} took {} against a {} budget",
                    format_duration(*seconds),
                    format_duration(budget(stage).unwrap_or_default())
                ))
                .collect::<Vec<_>>()
                .join("; ")
        )),
    }
    if let Some((tasks, median, task, worst)) = &evidence.commit_to_main {
        output.push_str(&format!(
            "Commit to main, from each task's park to the main commit that landed it: {tasks} tasks, median {}, worst {task} at {}. The worst task is budgeted as commit-to-main.\n\n",
            format_duration(*median),
            format_duration(*worst)
        ));
    }
    for (stage, seconds) in &evidence.stage_seconds {
        let limit = budget(stage);
        let status = match limit {
            Some(limit) if *seconds > limit => format!("FAILED, budget {}", format_duration(limit)),
            Some(limit) => format!("ok, budget {}", format_duration(limit)),
            None => "no budget".to_string(),
        };
        let trend = evidence
            .stage_trend
            .iter()
            .filter_map(|(tag, stages)| {
                stages
                    .iter()
                    .find(|(name, _)| name == stage)
                    .map(|(_, seconds)| format!("{tag} {}", format_duration(*seconds)))
            })
            .collect::<Vec<_>>();
        output.push_str(&format!(
            "- {stage}: {} ({status}); earlier releases, oldest first: {}\n",
            format_duration(*seconds),
            if trend.is_empty() {
                "unavailable".to_string()
            } else {
                trend.join(" · ")
            }
        ));
    }
    output.push('\n');
    output
}

/// Earlier releases' STAGE_SECS from their latency receipts, oldest first.
fn stage_trend(files: &[PathBuf], tag: &str, limit: usize) -> Vec<(String, Vec<(String, i64)>)> {
    let version = |tag: &str| -> Option<(u64, u64, u64)> {
        let mut parts = tag
            .trim_start_matches('v')
            .split('.')
            .map(|part| part.parse().ok());
        Some((parts.next()??, parts.next()??, parts.next()??))
    };
    let Some(current) = version(tag) else {
        return Vec::new();
    };
    let mut releases: Vec<((u64, u64, u64), String, Vec<(String, i64)>)> = files
        .iter()
        .filter(|path| {
            path.file_name().and_then(|name| name.to_str()) == Some("release-latency.receipt")
        })
        .filter_map(|path| {
            let values = parse_key_values(&fs::read_to_string(path).ok()?);
            let tag = values.get("TAG")?.clone();
            let stages = parse_stage_pairs(values.get("STAGE_SECS")?);
            let version = version(&tag).filter(|version| *version < current)?;
            (!stages.is_empty()).then_some((version, tag, stages))
        })
        .collect();
    releases.sort_by(|left, right| left.0.cmp(&right.0));
    releases.dedup_by(|left, right| left.0 == right.0);
    let skip = releases.len().saturating_sub(limit);
    releases
        .into_iter()
        .skip(skip)
        .map(|(_, tag, stages)| (tag, stages))
        .collect()
}

/// `stage:secs[+],…` from the latency receipt's BLOCKER_COSTS; a trailing `+`
/// marks a blocker still open at publication.
fn parse_blocker_costs(costs: &str) -> Vec<(String, i64, bool)> {
    costs
        .split(',')
        .filter_map(|item| {
            let (stage, seconds) = item.trim().rsplit_once(':')?;
            let (seconds, resolved) = match seconds.strip_suffix('+') {
                Some(open) => (open, false),
                None => (seconds, true),
            };
            let seconds = seconds
                .parse::<i64>()
                .ok()
                .filter(|seconds| *seconds >= 0)?;
            (!stage.is_empty()).then(|| (stage.to_string(), seconds, resolved))
        })
        .collect()
}

#[derive(Debug, Clone)]
struct AcquiredSources {
    project: String,
    changes: Vec<ShippedChange>,
    release_pr: Option<ReleasePr>,
    closure_window: Option<ClosureWindow>,
    issues_still_open: Vec<u64>,
    issues_closed_outside: Vec<u64>,
    changelog: Option<ChangelogSection>,
    changelog_path: Option<PathBuf>,
    release_notes: Option<(PathBuf, String)>,
    release_note_articles: Vec<ReleaseNoteArticle>,
    release_note_user_punch: Option<String>,
    release: Option<ReleaseMetadata>,
    issues: Vec<GithubIssue>,
    assets: Vec<ReleaseAsset>,
    release_evidence: ReleaseEvidence,
    github_repo: Option<String>,
    issue_count: usize,
    warnings: Vec<String>,
    retrieved_at: String,
}

#[derive(Debug, Clone)]
struct ReleaseNoteArticle {
    title: String,
    was: String,
    now: String,
    user_facing: bool,
    group: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct ReleaseNoteDraft {
    articles: Vec<ReleaseNoteArticle>,
    user_punch: Option<String>,
}

/// Execute `cas release report`.
pub fn execute(args: &ReleaseReportArgs, cli: &Cli) -> anyhow::Result<()> {
    let version = normalize_version(&args.version)?;
    let tag = format!("v{version}");
    let project_root = std::env::current_dir().context("could not determine project root")?;
    // A git worktree has its own checkout path but shares the project's .cas
    // directory with the main checkout. Resolve that root once and use it for
    // all project-scoped configuration reads below.
    let config_root = find_cas_root_from(&project_root).ok();
    let project_config = config_root
        .as_deref()
        .and_then(|root| Config::load(root).ok())
        .unwrap_or_default();
    let project = project_name(&project_config, &project_root);
    let out_dir = if args.out.is_absolute() {
        args.out.clone()
    } else {
        project_root.join(&args.out)
    };
    fs::create_dir_all(&out_dir)
        .with_context(|| format!("could not create report directory {}", out_dir.display()))?;

    let source_path = out_dir.join(format!("{tag}.md"));
    let html_path = out_dir.join(format!("{tag}.html"));
    let pdf_path = out_dir.join(format!("{tag}.pdf"));

    let (source, mut acquired, source_written) = if source_path.is_file() && !args.refresh_sources {
        let source = fs::read_to_string(&source_path)
            .with_context(|| format!("could not read existing source {}", source_path.display()))?;
        let acquired = AcquiredSources::from_existing(&source_path, &source, &tag, project.clone());
        (source, acquired, false)
    } else {
        let latency_receipt = args
            .latency_receipt
            .as_deref()
            .map(|path| standalone_latency_receipt(&project_root, path, &tag))
            .transpose()?;
        let acquired = acquire_sources(
            &project_root,
            &version,
            &tag,
            &project_config,
            project,
            latency_receipt.as_deref(),
        )?;
        let source = assemble_markdown(&project_root, &version, &tag, &acquired);
        write_text(&source_path, &source)?;
        (source, acquired, true)
    };

    let renderer = Workspace::materialize(&[
        Asset::builtin(RENDER_SCRIPT, "scripts/render.py")?,
        Asset::builtin(RENDER_TEMPLATE, "references/template.html")?,
    ])?;
    let relative = |path: &Path| {
        path.strip_prefix(&project_root)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    };
    let default_branch = crate::worktree::GitOperations::new(project_root.clone())
        .detect_default_branch();
    let links = report_link_hrefs(
        project_config.issue_repo_registry_for_project(&project_root).project.as_deref(),
        &default_branch,
        &relative(&source_path),
        &relative(&pdf_path),
    );
    let draft = renderer.path(&format!("{tag}.html"));
    render_html(
        &renderer.path("scripts/render.py"),
        &source_path,
        &draft,
        &project_root,
        RENDER_TIMEOUT,
        links,
    )?;
    // The renderer's draft becomes the report through the deliverables core:
    // brand fill, inline budget, audience hygiene, then A4 and Letter checked.
    let pdf_dir = renderer.path("pdf");
    let finalized = finalize::finalize(&FinalizeRequest {
        draft: draft.clone(),
        out: html_path.clone(),
        audience: args.audience,
        root: project_root.clone(),
        surface: None,
        tokens: None,
        allow: Vec::new(),
        budget: Budget::for_audience(args.audience),
        papers: if args.pdf { vec![Paper::A4, Paper::Letter] } else { Vec::new() },
        pdf_dir: Some(pdf_dir.clone()),
        check_dir: None,
        author: None,
    })?;
    for warning in &finalized.warnings {
        eprintln!("release report: {warning}");
    }
    if !finalized.findings.is_empty() {
        let reasons: Vec<String> = finalized
            .findings
            .iter()
            .map(|finding| format!("{} ({})", finding.message, finding.code))
            .collect();
        bail!("release report refused by `cas deliverable finalize`: {}", reasons.join("; "));
    }
    let mut pdf_written = false;
    if args.pdf {
        let a4 = pdf_dir.join(format!("{tag}-A4.pdf"));
        fs::copy(&a4, &pdf_path)
            .with_context(|| format!("could not copy PDF to {}", pdf_path.display()))?;
        pdf_written = true;
    }

    if !source_written {
        // An existing source is authoritative.  Its current issue/theme data
        // is intentionally not guessed from a second source fetch.
        acquired.issue_count = count_issues_in_source(&source);
        acquired.assets = Vec::new();
    }

    let result = ReportResult {
        version: version.clone(),
        tag,
        project: acquired.project.clone(),
        source_path: display_path(&source_path, &project_root),
        html_path: display_path(&html_path, &project_root),
        pdf_path: args.pdf.then(|| display_path(&pdf_path, &project_root)),
        source_written,
        html_written: true,
        pdf_written,
        issue_count: acquired.issue_count,
        asset_count: acquired.assets.len(),
        change_count: if source_written {
            acquired.changes.len()
        } else {
            count_changes_in_source(&source)
        },
        change_areas: area_counts(&acquired.changes),
        warnings: acquired.warnings.clone(),
        retrieved_at: acquired.retrieved_at.clone(),
        github_repo: acquired.github_repo.clone(),
        release_published_at: acquired
            .release
            .as_ref()
            .and_then(|release| release.published_at.clone())
            .or(acquired.release_evidence.tag_published_at.clone()),
        green_to_published_seconds: acquired.release_evidence.green_to_published_seconds,
        brand_source: finalized.brand_source.clone().unwrap_or_default(),
        design: finalized.design.clone(),
    };

    if cli.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        let stdout = io::stdout();
        let mut output = stdout.lock();
        let mut fmt = Formatter::stdout(&mut output, ActiveTheme::default());
        render_human_output(&mut fmt, &result)?;
        fmt.flush()?;
    }

    Ok(())
}

fn render_human_output(fmt: &mut Formatter<'_>, result: &ReportResult) -> io::Result<()> {
    let warning_count = result.warnings.len();
    let verdict = if warning_count == 0 {
        Verdict::Ok
    } else {
        Verdict::Warning
    };
    let separator = if fmt.unicode() { "·" } else { "-" };
    fmt.verdict(
        verdict,
        "report ready",
        &format!(
            "{} {separator} {} change{} {separator} {} GitHub issue{} closed {separator} {} warning{}",
            result.tag,
            result.change_count,
            if result.change_count == 1 { "" } else { "s" },
            result.issue_count,
            if result.issue_count == 1 { "" } else { "s" },
            warning_count,
            if warning_count == 1 { "" } else { "s" }
        ),
    )?;

    let width = report_output_width(fmt);
    write_wrapped_field(fmt, "  Source: ", &result.source_path, width)?;
    write_wrapped_field(fmt, "  HTML:  ", &result.html_path, width)?;
    if let Some(path) = &result.pdf_path {
        write_wrapped_field(fmt, "  PDF:   ", path, width)?;
    }
    if !result.warnings.is_empty() {
        write_wrapped_field(
            fmt,
            "  Remedy: ",
            "inspect the Evidence and scope section; rerun with --refresh-sources after fixing sources.",
            width,
        )?;
        for warning in &result.warnings {
            write_wrapped_warning(fmt, warning, width)?;
        }
    }
    Ok(())
}

fn report_output_width(fmt: &Formatter<'_>) -> usize {
    let width = fmt.width() as usize;
    if width < 40 { 80 } else { width }
}

/// Wrap ordinary words and hard-wrap a single long value (usually an absolute
/// path) with one spare cell. The spare cell avoids terminal-qa mistaking a
/// continuation of a path for a word split at the right edge.
fn wrap_report_output(text: &str, width: usize) -> Vec<String> {
    crate::ui::word_wrap::wrap_words(
        text,
        width,
        |text| text.chars().count(),
        crate::ui::word_wrap::OverlongWordPolicy::HardWrap { spare_columns: 1 },
    )
    .lines
}

fn output_text_for_locale(fmt: &Formatter<'_>, text: &str) -> String {
    if fmt.unicode() {
        text.to_string()
    } else {
        ascii_fallback(text).into_owned()
    }
}

fn write_wrapped_field(
    fmt: &mut Formatter<'_>,
    prefix: &str,
    text: &str,
    width: usize,
) -> io::Result<()> {
    let prefix_width = prefix.chars().count();
    let available = width.saturating_sub(prefix_width).max(1);
    let text = output_text_for_locale(fmt, text);
    for (index, line) in wrap_report_output(&text, available).into_iter().enumerate() {
        if index == 0 {
            fmt.write_raw(prefix)?;
        } else {
            fmt.write_raw(&" ".repeat(prefix_width))?;
        }
        fmt.write_text(&line)?;
        fmt.newline()?;
    }
    Ok(())
}

fn write_wrapped_warning(fmt: &mut Formatter<'_>, warning: &str, width: usize) -> io::Result<()> {
    let mark = Verdict::Warning.label(fmt.glyphs());
    let prefix_width = 2 + mark.chars().count() + 1;
    let available = width.saturating_sub(prefix_width).max(1);
    let warning = output_text_for_locale(fmt, warning);
    for (index, line) in wrap_report_output(&warning, available)
        .into_iter()
        .enumerate()
    {
        if index == 0 {
            fmt.write_raw("  ")?;
            fmt.mark(Verdict::Warning)?;
            fmt.write_raw(" ")?;
        } else {
            fmt.write_raw(&" ".repeat(prefix_width))?;
        }
        fmt.write_text(&line)?;
        fmt.newline()?;
    }
    Ok(())
}

impl AcquiredSources {
    fn from_existing(path: &Path, source: &str, tag: &str, project: String) -> Self {
        let issue_count = count_issues_in_source(source);
        Self {
            project,
            changes: Vec::new(),
            release_pr: None,
            closure_window: None,
            issues_still_open: Vec::new(),
            issues_closed_outside: Vec::new(),
            changelog: None,
            changelog_path: None,
            release_notes: None,
            release_note_articles: Vec::new(),
            release_note_user_punch: None,
            release: None,
            issues: Vec::new(),
            assets: Vec::new(),
            release_evidence: ReleaseEvidence::default(),
            github_repo: None,
            issue_count: 0,
            warnings: vec![format!(
                "source preserved: {} is authoritative for {tag}",
                path.display()
            )],
            retrieved_at: Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        }
        .with_issue_count(issue_count)
    }

    fn with_issue_count(mut self, issue_count: usize) -> Self {
        // Existing Markdown is rendered as-is. Keep its count available to
        // JSON without pretending that its ledger is newly fetched.
        self.issue_count = issue_count;
        self
    }
}

fn acquire_sources(
    project_root: &Path,
    version: &str,
    tag: &str,
    config: &Config,
    project: String,
    latency_receipt: Option<&Path>,
) -> anyhow::Result<AcquiredSources> {
    let retrieved_at = Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut warnings = Vec::new();

    let (changelog, changelog_path) = match find_changelog(project_root) {
        Some(path) => {
            let content = fs::read_to_string(&path)
                .with_context(|| format!("could not read {}", path.display()))?;
            match parse_changelog_section(&content, version) {
                Some(section) => (Some(section), Some(path)),
                None => {
                    warnings.push(format!(
                        "CHANGELOG: no Keep-a-Changelog section for {tag}; add `## [{version}] - YYYY-MM-DD`"
                    ));
                    (None, Some(path))
                }
            }
        }
        None => {
            warnings.push("CHANGELOG: CHANGELOG.md is unavailable".to_string());
            (None, None)
        }
    };

    let release_notes = find_release_notes(project_root, version);
    let release_note_draft = release_notes
        .as_ref()
        .map(|(_, notes)| parse_release_note_draft(notes))
        .unwrap_or_default();
    if release_notes.is_none() {
        warnings.push(format!(
            "release notes: no draft matching {tag} under docs/release-notes/"
        ));
    }

    let github_repo = config.issue_repo_registry_for_project(project_root).project;
    let mut release = None;
    let mut issues = Vec::new();
    let mut assets = Vec::new();
    let mut release_pr = None;
    let mut counting_window = None;
    let mut issues_still_open = Vec::new();
    let mut issues_closed_outside = Vec::new();

    if let Some(repo) = &github_repo {
        match gh_json(
            project_root,
            &[
                "release",
                "view",
                tag,
                "--repo",
                repo,
                "--json",
                "name,url,body,publishedAt,isDraft,assets",
            ],
        ) {
            Ok(value) => {
                let metadata = parse_release_metadata(&value, repo);
                assets = metadata.assets.clone();
                release = Some(metadata);
            }
            Err(error) => warnings.push(format!(
                "GitHub release: {error}; remedy `gh release view {tag} --repo {repo}`"
            )),
        }

        let mut referenced = BTreeSet::new();
        if let Some(section) = &changelog {
            referenced.extend(extract_issue_numbers(&section.body));
        }
        if let Some((_, notes)) = &release_notes {
            referenced.extend(extract_issue_numbers(notes));
        }
        if let Some(metadata) = &release {
            if let Some(body) = &metadata.body {
                // Release bodies commonly mention the release PR itself.  A
                // URL explicitly under /issues/ is unambiguously an issue;
                // closing issues from the PR are acquired below.
                referenced.extend(extract_issue_url_numbers(body));
            }
        }

        match fetch_release_pr(project_root, repo, version) {
            Ok(pr) => release_pr = pr,
            Err(error) => warnings.push(format!(
                "GitHub release PRs: {error}; remedy `gh pr list --repo {repo} --state merged`"
            )),
        }
        match &release_pr {
            Some(pr) => referenced.extend(pr.referenced.iter().copied()),
            None => warnings.push(format!(
                "GitHub release PR: no merged PR titled for {tag}; issue closures are counted without a time window"
            )),
        }
        counting_window = release_pr.as_ref().map(|pr| ClosureWindow {
            opened: pr.created_at,
            closes: pr.merged_at + closure_window(),
        });

        let mut announced = false;
        let fetch = || {
            gh_json(
                project_root,
                &[
                    "issue",
                    "list",
                    "--repo",
                    repo,
                    "--state",
                    "all",
                    "--limit",
                    "1000",
                    "--json",
                    "number,title,url,state,closedAt,labels,body",
                ],
            )
        };
        let sleep = |pause: Duration| {
            if !announced {
                announced = true;
                eprintln!(
                    "release report: waiting up to {CLOSURE_WINDOW_MINUTES} minutes after the release PR merge for referenced issues to close"
                );
            }
            std::thread::sleep(pause);
        };
        match gather_closures(fetch, &referenced, counting_window.as_ref(), repo, Utc::now, sleep) {
            Ok(snapshot) => {
                issues = snapshot.closed;
                for issue in &mut issues {
                    issue.theme = classify_area(&issue.title, "");
                }
                issues_still_open = snapshot.still_open;
                issues_closed_outside = snapshot.closed_outside;
                if !snapshot.missing.is_empty() {
                    warnings.push(format!(
                        "GitHub issues: referenced issues not found: {}",
                        snapshot
                            .missing
                            .iter()
                            .map(|number| format!("#{number}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            Err(error) => warnings.push(format!(
                "GitHub issues: {error}; remedy `gh issue list --repo {repo} --state all`"
            )),
        }
    } else {
        warnings.push(
            "GitHub sources: issues.repo is unset; remedy `cas config set issues.repo owner/name`"
                .to_string(),
        );
    }

    let release_evidence = find_release_evidence(tag, &release, latency_receipt, config);
    if release_evidence
        .budget_breaches
        .as_ref()
        .is_some_and(|breaches| !breaches.is_empty())
    {
        warnings.push(format!(
            "pipeline latency: FAILED, over budget: {}",
            release_evidence
                .budget_breaches
                .as_deref()
                .unwrap_or_default()
                .join(", ")
        ));
    }
    if !release_evidence.has_reportable_publication_timing() {
        warnings.push(format!(
            "release receipts: green-to-published latency is unavailable under {}/release/; pass --latency-receipt <run-dir>/release-latency.receipt for a standalone measurement",
            release_artifacts_root(config).1.trim_end_matches('/')
        ));
    }

    let issue_count = issues.len();
    let changes = changelog
        .as_ref()
        .map(|section| changelog_changes(&section.body))
        .unwrap_or_default();
    Ok(AcquiredSources {
        project,
        changes,
        release_pr,
        closure_window: counting_window,
        issues_still_open,
        issues_closed_outside,
        changelog,
        changelog_path,
        release_notes,
        release_note_articles: release_note_draft.articles,
        release_note_user_punch: release_note_draft.user_punch,
        release,
        issues,
        assets,
        release_evidence,
        github_repo,
        issue_count,
        warnings,
        retrieved_at,
    })
}

fn parse_changelog_section(content: &str, version: &str) -> Option<ChangelogSection> {
    let wanted = version.trim_start_matches('v');
    let lines = content.lines().collect::<Vec<_>>();
    let start = lines.iter().position(|line| {
        let heading = line.trim();
        if !heading.starts_with("## ") {
            return false;
        }
        let rest = heading.trim_start_matches("## ").trim();
        let candidate = rest
            .strip_prefix('[')
            .and_then(|rest| rest.split_once(']'))
            .map(|(value, _)| value)
            .unwrap_or_else(|| rest.split_whitespace().next().unwrap_or(rest));
        candidate.trim_start_matches('v') == wanted
    })?;
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| line.trim_start().starts_with("## "))
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    let heading = lines[start].trim().trim_start_matches("## ").to_string();
    let date = Regex::new(r"\b(20\d{2}-\d{2}-\d{2})\b")
        .ok()
        .and_then(|regex| regex.captures(&heading))
        .and_then(|capture| capture.get(1).map(|match_| match_.as_str().to_string()));
    let body = lines[start + 1..end].join("\n").trim().to_string();
    let mut category = String::new();
    let mut entries = Vec::new();
    let mut current = None;
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("### ") {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            category = value
                .split_once(" — ")
                .map(|(kind, _)| kind)
                .unwrap_or(value)
                .trim()
                .to_string();
        } else if let Some(value) = line.trim_start().strip_prefix("- ") {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            let text = value.trim();
            if !text.is_empty() {
                current = Some(ChangelogEntry {
                    category: if category.is_empty() {
                        "Changes".to_string()
                    } else {
                        category.clone()
                    },
                    text: text.to_string(),
                });
            }
        } else if let Some(entry) = current.as_mut() {
            // Keep-a-Changelog permits a bullet's prose to wrap onto indented
            // lines. Preserve the complete entry for issue extraction and the
            // report's Was/Now fallback instead of silently truncating it.
            let continuation = trimmed;
            if !continuation.is_empty() {
                entry.text.push(' ');
                entry.text.push_str(continuation);
            }
        }
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    Some(ChangelogSection {
        heading,
        date,
        body,
        entries,
    })
}

/// One shipped change from a CHANGELOG section, as page one counts it.
#[derive(Debug, Clone, Serialize)]
struct ShippedChange {
    kind: String,
    title: String,
    area: String,
}

/// Product areas in their fixed page-one order, with the words that place a
/// change in each. A change goes to the area with the most matching words in
/// its title, then in its full text; ties go to the earlier area.
const AREAS: [(&str, &[&str]); 8] = [
    ("Commander", &["commander", "hub", "rail", "browser", "phone"]),
    ("Sessions", &[
        "session", "worker", "supervisor", "factory", "agent", "heartbeat", "relay",
        "alert", "spawn", "daemon", "typing", "keystroke", "tool call", "cas serve",
        "peer", "prompt", "nudge", "respawn", "stall",
    ]),
    ("Tasks", &[
        "task", "delivery", "deliveries", "merge request", "merges", "parked", "close gate",
        "qa", "verification", "epic", "review", "proof", "receipt",
    ]),
    ("Messages", &["message", "reminder", "inbox", "reply", "replies", "notice"]),
    ("Releases", &[
        "release", "merge queue", "ci", "install", "update", "changelog", "publish",
        "publication",
        "build", "gate", "runner", "journey",
    ]),
    ("Memory", &["memory", "recall", "search", "index", "briefing", "lesson"]),
    ("Slack and Cloud", &["slack", "violet", "cloud", "sync", "pairing"]),
    ("Safety", &[
        "permission", "operator-authenticated", "operator-policy", "security", "secret",
        "credential", "allowlist", "authenticated",
    ]),
];
const OTHER_AREA: &str = "Other";

fn area_score(text: &str, words: &[&str]) -> usize {
    words
        .iter()
        .filter(|word| {
            Regex::new(&format!(
                r"(^|[^a-z0-9]){}(s|es|d|ed|ing)?([^a-z0-9]|$)",
                regex::escape(&word.replace('-', " "))
            ))
                .is_ok_and(|regex| regex.is_match(text))
        })
        .count()
}

fn classify_area(title: &str, body: &str) -> String {
    // "merge-queue" and "merge queue" are the same words.
    for text in [title.to_string(), format!("{title} {body}")] {
        let text = text.to_ascii_lowercase().replace('-', " ");
        let mut best: Option<(&str, usize)> = None;
        for (area, words) in AREAS {
            let score = area_score(&text, words);
            if score > 0 && best.is_none_or(|(_, top)| score > top) {
                best = Some((area, score));
            }
        }
        if let Some((area, _)) = best {
            return area.to_string();
        }
    }
    OTHER_AREA.to_string()
}

/// CHANGELOG categories that record tests and documentation, not product
/// changes; reports list them under the hood but do not count them.
fn is_test_or_docs_category(category: &str) -> bool {
    let category = category.to_ascii_lowercase();
    category.starts_with("test") || category.starts_with("doc")
}

/// Strip trailing reference lists such as `(#1203, cas-d697)` from a title.
fn plain_change_title(title: &str) -> String {
    let references = Regex::new(r"\s*\([^()]*(#\d+|cas-[0-9a-f]+)[^()]*\)\s*$")
        .expect("reference suffix regex");
    let mut title = clean_markdown_text(title).replace("&#124;", "/");
    while let Some(found) = references.find(&title) {
        title.truncate(found.start());
    }
    let title = title
        .trim()
        .trim_matches('*')
        .trim()
        .replace(';', ",");
    let mut chars = title.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => "Release change".to_string(),
    }
}

/// The shipped changes in one CHANGELOG section. A titled heading
/// (`### Fixed — what changed (refs)`) is one change unless top-level
/// bold-titled entries starting with the first entry identify it as a group.
/// Under a group or an untitled heading each bullet is one change, titled
/// by its bold lead-in.
fn changelog_changes(body: &str) -> Vec<ShippedChange> {
    let mut changes = Vec::new();
    let mut kind = "Changes".to_string();
    let mut titled: Option<(String, String)> = None;
    let mut titled_has_entry = false;
    let mut bullet: Option<String> = None;
    let bold_title = Regex::new(r"^\*\*([^*]+)\*\*").expect("bold change title regex");
    let flush_bullet = |bullet: &mut Option<String>, kind: &str, changes: &mut Vec<ShippedChange>| {
        if let Some(text) = bullet.take() {
            let title = bold_title.captures(&text)
                .and_then(|capture| capture.get(1))
                .map(|lead| lead.as_str().to_string())
                .unwrap_or_else(|| heading_from_entry(&clean_markdown_text(&text)));
            changes.push(ShippedChange {
                kind: kind.to_string(),
                area: classify_area(&title, &text),
                title: plain_change_title(&title),
            });
        }
    };
    let flush_titled = |titled: &mut Option<(String, String)>, kind: &str, changes: &mut Vec<ShippedChange>| {
        if let Some((title, text)) = titled.take() {
            changes.push(ShippedChange {
                kind: kind.to_string(),
                area: classify_area(&plain_change_title(&title), &text),
                title: plain_change_title(&title),
            });
        }
    };
    for line in body.lines() {
        let trimmed = line.trim();
        // A group's title is context, not an extra shipped change. Only the
        // first top-level entry can switch modes; a later bold detail retains
        // the legacy single-change heading and its earlier plain entries.
        if titled.is_some()
            && !titled_has_entry
            && line.strip_prefix("- ").is_some_and(|entry| bold_title.is_match(entry))
        {
            titled = None;
        }
        if let Some(heading) = trimmed.strip_prefix("### ") {
            flush_bullet(&mut bullet, &kind, &mut changes);
            flush_titled(&mut titled, &kind, &mut changes);
            let (category, title) = heading
                .split_once(" — ")
                .map(|(category, title)| (category.trim(), Some(title.trim())))
                .unwrap_or((heading.trim(), None));
            kind = category.to_string();
            if is_test_or_docs_category(&kind) {
                continue;
            }
            titled = title.map(|title| (title.to_string(), String::new()));
            titled_has_entry = false;
        } else if is_test_or_docs_category(&kind) {
            continue;
        } else if let Some((_, text)) = titled.as_mut() {
            titled_has_entry |= line.starts_with("- ");
            text.push(' ');
            text.push_str(trimmed);
        } else if let Some(value) = line.trim_start().strip_prefix("- ") {
            flush_bullet(&mut bullet, &kind, &mut changes);
            bullet = Some(value.trim().to_string());
        } else if let Some(text) = bullet.as_mut() {
            if !trimmed.is_empty() {
                text.push(' ');
                text.push_str(trimmed);
            }
        }
    }
    flush_bullet(&mut bullet, &kind, &mut changes);
    flush_titled(&mut titled, &kind, &mut changes);
    changes
}

/// Changes per area, in the fixed area order; areas without a change are
/// left out so page one never shows a zero row.
#[derive(Debug, Clone, Serialize)]
struct AreaCount {
    area: String,
    changes: usize,
    titles: Vec<String>,
}

fn area_counts(changes: &[ShippedChange]) -> Vec<AreaCount> {
    AREAS
        .iter()
        .map(|(area, _)| *area)
        .chain([OTHER_AREA])
        .filter_map(|area| {
            let titles = changes
                .iter()
                .filter(|change| change.area == area)
                .map(|change| change.title.clone())
                .collect::<Vec<_>>();
            (!titles.is_empty()).then(|| AreaCount {
                area: area.to_string(),
                changes: titles.len(),
                titles,
            })
        })
        .collect()
}

fn parse_issues(value: &Value, referenced: &BTreeSet<u64>, repo: &str) -> Vec<GithubIssue> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|raw| {
            let number = raw.get("number").and_then(Value::as_u64)?;
            if !referenced.contains(&number) {
                return None;
            }
            let title = raw
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_else(|| "Issue")
                .trim()
                .to_string();
            let url = raw
                .get("url")
                .and_then(Value::as_str)
                .filter(|url| !url.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("https://github.com/{repo}/issues/{number}"));
            let labels = raw
                .get("labels")
                .and_then(Value::as_array)
                .map(|labels| {
                    labels
                        .iter()
                        .filter_map(|label| {
                            label
                                .get("name")
                                .and_then(Value::as_str)
                                .or_else(|| label.as_str())
                                .map(str::to_string)
                        })
                        .collect()
                })
                .unwrap_or_default();
            Some(GithubIssue {
                number,
                title,
                url,
                state: raw
                    .get("state")
                    .and_then(Value::as_str)
                    .unwrap_or("CLOSED")
                    .to_string(),
                closed_at: raw
                    .get("closedAt")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                labels,
                body: raw
                    .get("body")
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned),
                theme: String::new(),
            })
        })
        .collect()
}

fn parse_release_metadata(value: &Value, repo: &str) -> ReleaseMetadata {
    let assets = value
        .get("assets")
        .and_then(Value::as_array)
        .map(|assets| {
            assets
                .iter()
                .filter_map(|asset| {
                    Some(ReleaseAsset {
                        name: asset.get("name")?.as_str()?.to_string(),
                        size: asset.get("size").and_then(Value::as_u64),
                        url: asset
                            .get("url")
                            .or_else(|| asset.get("apiUrl"))
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned),
                        digest: asset
                            .get("digest")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned),
                        content_type: asset
                            .get("contentType")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    ReleaseMetadata {
        name: value
            .get("name")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        url: value
            .get("url")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| Some(format!("https://github.com/{repo}/releases"))),
        body: value
            .get("body")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        published_at: value
            .get("publishedAt")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        is_draft: value.get("isDraft").and_then(Value::as_bool),
        assets,
    }
}

fn collect_pr_issue_references(value: &Value, referenced: &mut BTreeSet<u64>) {
    let Some(prs) = value.as_array() else {
        return;
    };
    for pr in prs {
        if let Some(closures) = pr.get("closingIssuesReferences").and_then(Value::as_array) {
            for issue in closures {
                if let Some(number) = issue.get("number").and_then(Value::as_u64) {
                    referenced.insert(number);
                }
            }
        }
        if let Some(body) = pr.get("body").and_then(Value::as_str) {
            referenced.extend(extract_issue_numbers(body));
        }
    }
}

/// The merged release PR for a version, and the issues it references.
#[derive(Debug, Clone)]
struct ReleasePr {
    number: u64,
    head_ref: Option<String>,
    url: Option<String>,
    created_at: Option<DateTime<Utc>>,
    merged_at: DateTime<Utc>,
    referenced: BTreeSet<u64>,
}

/// The span in which a closed issue belongs to this release.
#[derive(Debug, Clone)]
struct ClosureWindow {
    opened: Option<DateTime<Utc>>,
    closes: DateTime<Utc>,
}

/// One issue-state snapshot, split by what it means for this release.
#[derive(Debug, Clone, Default)]
struct ClosureSnapshot {
    closed: Vec<GithubIssue>,
    still_open: Vec<u64>,
    closed_outside: Vec<u64>,
    missing: Vec<u64>,
}

fn parse_utc(value: Option<&str>) -> Option<DateTime<Utc>> {
    value
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

/// Pick the merged release PR whose title names this version.
fn select_release_pr(value: &Value, version: &str) -> Option<ReleasePr> {
    let title_pattern = Regex::new(&format!(
        r"(?i)(^|[^0-9.])v?{}($|[^0-9])",
        regex::escape(version)
    ))
    .ok()?;
    value
        .as_array()?
        .iter()
        .filter(|pr| {
            pr.get("title")
                .and_then(Value::as_str)
                .is_some_and(|title| {
                    title.to_ascii_lowercase().contains("release") && title_pattern.is_match(title)
                })
        })
        .filter_map(|pr| {
            let merged_at = parse_utc(pr.get("mergedAt").and_then(Value::as_str))?;
            let mut referenced = BTreeSet::new();
            collect_pr_issue_references(&Value::Array(vec![pr.clone()]), &mut referenced);
            Some(ReleasePr {
                number: pr.get("number").and_then(Value::as_u64)?,
                head_ref: pr.get("headRefName").and_then(Value::as_str).map(ToOwned::to_owned),
                url: pr.get("url").and_then(Value::as_str).map(ToOwned::to_owned),
                created_at: parse_utc(pr.get("createdAt").and_then(Value::as_str)),
                merged_at,
                referenced,
            })
        })
        .min_by_key(|pr| pr.merged_at)
}

/// The same version-bound merged PR drives report evidence and upload ownership.
fn fetch_release_pr(project_root: &Path, repo: &str, version: &str) -> anyhow::Result<Option<ReleasePr>> {
    let tag = format!("v{version}");
    for search in [tag.as_str(), version] {
        let value = gh_json(project_root, &[
            "pr", "list", "--repo", repo, "--state", "merged", "--search", search,
            "--limit", "30", "--json",
            "number,title,body,url,closingIssuesReferences,createdAt,mergedAt,headRefName",
        ])?;
        if let Some(pr) = select_release_pr(&value, version) {
            return Ok(Some(pr));
        }
    }
    Ok(None)
}

/// Recognize generated report PDFs, including custom output directories with
/// their generated Markdown companion. Other files keep normal publication
/// context; a version-looking filename alone does not confer release identity.
fn report_pdf_version(path: &Path) -> Option<String> {
    let path = fs::canonicalize(path).ok()?;
    if path.extension()?.to_str()? != "pdf" {
        return None;
    }
    let version = normalize_version(path.file_stem()?.to_str()?.strip_prefix('v')?).ok()?;
    let report_directory = path.parent()?.file_name()?.to_str()? == "release-reports";
    let generated_source = || fs::read_to_string(path.with_extension("md")).ok().is_some_and(|source| {
        source.lines().next().is_some_and(|title| title.starts_with("# ") && title.ends_with(&format!(" v{version}")))
            && source.contains("\n## Evidence and scope\n")
    });
    (report_directory || generated_source()).then_some(version)
}

/// File uploads of one release use its PR's durable epic lane, independently
/// of session focus. Missing/ambiguous evidence preserves the ordinary gate;
/// callers can explicitly name --epic. Mixed attachments never borrow a PDF's
/// release identity for an unrelated deliverable.
pub(crate) fn publication_epic_for_reports(cas_root: &Path, files: &[PathBuf]) -> Option<String> {
    let first = report_pdf_version(files.first()?)?;
    if files.iter().skip(1).any(|path| report_pdf_version(path).as_deref() != Some(&first)) {
        return None;
    }
    let project = cas_root.parent()?;
    let config = Config::load(cas_root).ok()?;
    let repo = config.issue_repo_registry_for_project(project).project?;
    let branch = fetch_release_pr(project, &repo, &first).ok()??.head_ref?;
    let store = crate::store::open_task_store(cas_root).ok()?;
    let mut epics = store.list(None).ok()?.into_iter().filter(|task| {
        task.task_type == cas_types::TaskType::Epic
            && task.effective_epic_branch() == Some(branch.as_str())
    });
    let epic = epics.next()?;
    epics.next().is_none().then_some(epic.id)
}

/// Split one snapshot of issue states into this release's closures.
fn classify_closures(
    value: &Value,
    candidates: &BTreeSet<u64>,
    window: Option<&ClosureWindow>,
    repo: &str,
) -> ClosureSnapshot {
    let mut snapshot = ClosureSnapshot::default();
    for issue in parse_issues(value, candidates, repo) {
        if !issue.state.eq_ignore_ascii_case("closed") {
            snapshot.still_open.push(issue.number);
            continue;
        }
        // Without a release PR there is no window; keep the referenced
        // closure and let Evidence and scope say so.
        let inside = window.is_none_or(|window| {
            parse_utc(issue.closed_at.as_deref()).is_some_and(|closed| {
                closed <= window.closes && window.opened.is_none_or(|opened| closed >= opened)
            })
        });
        if inside {
            snapshot.closed.push(issue);
        } else {
            snapshot.closed_outside.push(issue.number);
        }
    }
    snapshot.closed.sort_by_key(|issue| issue.number);
    snapshot.still_open.sort_unstable();
    snapshot.closed_outside.sort_unstable();
    snapshot.missing = candidates
        .iter()
        .filter(|number| {
            !snapshot.closed.iter().any(|issue| issue.number == **number)
                && !snapshot.still_open.contains(number)
                && !snapshot.closed_outside.contains(number)
        })
        .copied()
        .collect();
    snapshot
}

/// Read issue states and classify this release's closures.
///
/// Referenced issues are closed by hand a few minutes after the release PR
/// merges, usually after a report run has started. While the closure window
/// is still open and a referenced issue is still open, re-read the states
/// until it closes or the window ends, so the count needs no hand correction.
fn gather_closures(
    mut fetch: impl FnMut() -> anyhow::Result<Value>,
    candidates: &BTreeSet<u64>,
    window: Option<&ClosureWindow>,
    repo: &str,
    mut now: impl FnMut() -> DateTime<Utc>,
    mut sleep: impl FnMut(Duration),
) -> anyhow::Result<ClosureSnapshot> {
    loop {
        let snapshot = classify_closures(&fetch()?, candidates, window, repo);
        let Some(window) = window else {
            return Ok(snapshot);
        };
        let remaining = window.closes - now();
        if snapshot.still_open.is_empty() || remaining <= chrono::Duration::zero() {
            return Ok(snapshot);
        }
        sleep(remaining.to_std().unwrap_or_default().min(closure_poll()));
    }
}

fn assemble_markdown(
    project_root: &Path,
    version: &str,
    tag: &str,
    sources: &AcquiredSources,
) -> String {
    let project = &sources.project;
    let date = sources
        .release
        .as_ref()
        .and_then(|release| release.published_at.as_deref())
        .and_then(date_from_timestamp)
        .or_else(|| {
            sources
                .changelog
                .as_ref()
                .and_then(|section| section.date.clone())
        })
        .unwrap_or_else(|| Utc::now().date_naive().to_string());
    let issue_count = sources.issues.len();
    let areas = area_counts(&sources.changes);
    let change_count = sources.changes.len();
    let plural = |count: usize, one: &str, many: &str| {
        format!("{count} {}", if count == 1 { one } else { many })
    };
    let area_frontmatter = format!(
        "[{}]",
        areas
            .iter()
            .map(|area| format!("\"{}\"", area.area))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let verdict = sources
        .release_note_user_punch
        .as_deref()
        .map(format_verdict)
        .unwrap_or_else(|| {
            if change_count == 0 {
                format!("{tag} has no listed changes in the available release sources.")
            } else {
                format!(
                    "{tag} ships {} across {}.",
                    plural(change_count, "change", "changes"),
                    plural(areas.len(), "area", "areas")
                )
            }
        });
    let publication_line = sources
        .release
        .as_ref()
        .and_then(|release| release.published_at.as_deref())
        .or(sources.release_evidence.tag_published_at.as_deref())
        .map(format_publication_line)
        .unwrap_or_else(|| format!("Draft assembled {date} · publication evidence unavailable"));

    let claim = match areas.as_slice() {
        [] => "No changes are listed for this release yet.".to_string(),
        [only] => format!(
            "{} shipped, all in {}.",
            plural(change_count, "change", "changes"),
            only.area
        ),
        _ if areas.len() <= 3 => {
            let parts = areas
                .iter()
                .map(|area| format!("{} in {}", area.changes, area.area))
                .collect::<Vec<_>>();
            let (last, rest) = parts.split_last().expect("at least two areas");
            format!(
                "{} shipped: {} and {last}.",
                plural(change_count, "change", "changes"),
                rest.join(", ")
            )
        }
        _ => {
            let top = areas
                .iter()
                .fold(&areas[0], |top, area| if area.changes > top.changes { area } else { top });
            format!(
                "{} shipped across {} areas; {} has the most, {}.",
                plural(change_count, "change", "changes"),
                areas.len(),
                top.area,
                top.changes
            )
        }
    };
    let guide = format!(
        "One dot is one change listed in the {tag} CHANGELOG. Dots count changes, not their size or importance."
    );
    let map_rows = areas
        .iter()
        .map(|area| {
            format!(
                "| {} | {} | {} |",
                area.area,
                area.changes,
                escape_table(&area.titles.join("; "))
            )
        })
        .collect::<Vec<_>>();
    let source_refs = source_reference_line(sources, tag, project_root);
    let release_pr_label = sources
        .release_pr
        .as_ref()
        .map(|pr| format!("PR #{}", pr.number));

    // Page one shows measured, nonzero values only; zeros and missing
    // measurements are named in the counting note instead.
    let mut glance = Vec::<(String, String, String)>::new();
    let mut zero_notes = Vec::<String>::new();
    let mut unavailable = Vec::<&str>::new();
    if change_count > 0 {
        let mut kinds = Vec::<(String, usize)>::new();
        for change in &sources.changes {
            let kind = change.kind.to_ascii_lowercase();
            match kinds.iter_mut().find(|(known, _)| *known == kind) {
                Some((_, count)) => *count += 1,
                None => kinds.push((kind, 1)),
            }
        }
        glance.push((
            "Changes shipped".to_string(),
            change_count.to_string(),
            format!(
                "{}; tests and docs not counted",
                kinds
                    .iter()
                    .map(|(kind, count)| format!("{count} {kind}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    } else {
        zero_notes.push("No changes are listed in the CHANGELOG section.".to_string());
    }
    if issue_count > 0 {
        glance.push((
            "GitHub issues closed".to_string(),
            issue_count.to_string(),
            match &release_pr_label {
                Some(pr) => format!("Closed from {pr} opening to {CLOSURE_WINDOW_MINUTES} min after merge"),
                None => "Referenced issues that are closed; no release PR found".to_string(),
            },
        ));
    } else {
        zero_notes.push("No GitHub issues are counted in this report's fixes ledger.".to_string());
    }
    let green_timing =
        if sources.release_evidence.publication_start_kind.as_deref() == Some("gate-green") {
            Some(sources.release_evidence.publication_timing())
        } else {
            sources
                .release_evidence
                .green_to_published_seconds
                .map(format_duration)
        };
    match green_timing.filter(|value| value != "unavailable") {
        Some(value) => glance.push((
            "Green to published".to_string(),
            value,
            "Merge-queue CI completion to publication".to_string(),
        )),
        None => unavailable.push("green to published"),
    }
    match sources.release_evidence.request_timing() {
        value if value == "unavailable" => unavailable.push("request to published"),
        value => glance.push((
            "Request to published".to_string(),
            value,
            "The release request, or first cut, to publication".to_string(),
        )),
    }
    match sources.release_evidence.cut_to_published_seconds {
        Some(seconds) => glance.push((
            "Cut to published".to_string(),
            format_duration(seconds),
            "The first release-train cut to publication".to_string(),
        )),
        None => unavailable.push("cut to published"),
    }
    if let Some((publication_endpoint, publication_description)) =
        sources.release_evidence.publication_endpoint()
    {
        let timing = sources.release_evidence.publication_timing();
        if timing != "unavailable" {
            glance.push((
                publication_endpoint.to_string(),
                timing,
                publication_description.to_string(),
            ));
        }
    }
    match sources.release_evidence.blocker_summary() {
        value if value == "unavailable" => unavailable.push("blockers"),
        value if value == "0" => zero_notes.push("No release stage was blocked.".to_string()),
        value => glance.push((
            "Blockers".to_string(),
            value,
            "Each blocked stage, from the block to its next completion".to_string(),
        )),
    }
    match sources.release_evidence.lesson_recurrence.as_deref() {
        None => unavailable.push("lesson recurrence"),
        Some(value) if value.trim() == "0" || value.trim().starts_with("0 ") => {
            zero_notes.push("No rescue repeated a lesson already learned.".to_string())
        }
        Some(value) => glance.push((
            "Lesson recurrence".to_string(),
            value.to_string(),
            "Rescues whose lesson was already in the failure log".to_string(),
        )),
    }
    match sources.release_evidence.interventions {
        None => unavailable.push("manual interventions"),
        Some(0) => zero_notes.push("No manual interventions.".to_string()),
        Some(count) => glance.push((
            "Manual interventions".to_string(),
            count.to_string(),
            "Rescued stages plus recorded hand fixes".to_string(),
        )),
    }
    // The stat strip shows a short figure; any qualifier joins its definition.
    let glance = glance
        .into_iter()
        .map(|(measure, value, definition)| {
            let split = value.split_once(" — ").or_else(|| {
                (measure == "Lesson recurrence")
                    .then(|| value.split_once(" ("))
                    .flatten()
            });
            match split {
                Some((figure, qualifier)) => {
                    let qualifier = if value.contains(" — ") {
                        let mut chars = qualifier.chars();
                        chars
                            .next()
                            .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                            .unwrap_or_default()
                    } else {
                        format!("({qualifier}")
                    };
                    (measure, figure.to_string(), format!("{qualifier}; {definition}"))
                }
                None => (measure, value, definition),
            }
        })
        .collect::<Vec<_>>();
    let mut method = vec![format!(
        "Changes and GitHub issues are counted separately. A change is one titled entry in the {tag} CHANGELOG section; tests and documentation entries are listed under the hood but not counted."
    )];
    match (&sources.release_pr, &sources.closure_window) {
        (Some(pr), Some(window)) => method.push(format!(
            "A GitHub issue counts when release PR #{} or the CHANGELOG section references it and it closed between the PR opening{} and {CLOSURE_WINDOW_MINUTES} minutes after its merge ({}). Issues are closed by hand shortly after the merge, so a report run inside that window waits for referenced issues that are still open.",
            pr.number,
            window
                .opened
                .map(|opened| format!(" ({})", eastern_time(opened)))
                .unwrap_or_default(),
            eastern_time(window.closes)
        )),
        _ => method.push(
            "No merged release PR was found, so every referenced closed issue counts.".to_string(),
        ),
    }
    method.extend(zero_notes);
    if !unavailable.is_empty() {
        method.push(format!(
            "Not measured for this release: {}.",
            unavailable.join(", ")
        ));
    }

    let mut output = String::new();
    output.push_str("---\n");
    output.push_str(&format!("version: {version}\n"));
    output.push_str(&format!("date: {date}\n"));
    output.push_str(&format!("areas: {area_frontmatter}\n"));
    output.push_str("---\n\n");
    output.push_str(&format!(
        "# {project} {tag}\n\n{publication_line}\n\n{verdict}\n\n"
    ));
    output.push_str("## Change map\n\n");
    output.push_str(&format!("{claim}\n\n{guide}\n\n"));
    output.push_str("| Area | Changes | What changed |\n| --- | ---: | --- |\n");
    for row in &map_rows {
        output.push_str(row);
        output.push('\n');
    }
    output.push_str(&format!(
        "| Total | {change_count} | {} |\n\n{source_refs}\n\n",
        plural(areas.len(), "area", "areas")
    ));
    output.push_str("## Release at a glance\n\n");
    output.push_str("| Measure | Value | Definition |\n| --- | ---: | --- |\n");
    for (measure, value, definition) in &glance {
        output.push_str(&format!(
            "| {measure} | {} | {} |\n",
            escape_table(value),
            escape_table(definition)
        ));
    }
    output.push('\n');
    output.push_str(&method.join(" "));
    output.push_str("\n\n");
    output.push_str("## What you can do now\n\n");
    output.push_str(&was_now_sections(sources, true));
    output.push_str("\n## Under the hood\n\n");
    output.push_str(&was_now_sections(sources, false));
    output.push_str("\n## Fixes ledger\n\n");
    output.push_str("| Issue | Title | Closed | Area |\n| --- | --- | --- | --- |\n");
    for issue in &sources.issues {
        output.push_str(&format!(
            "| [#{}]({}) | {} | {} | {} |\n",
            issue.number,
            issue.url,
            escape_table(&issue.title),
            issue
                .closed_at
                .as_deref()
                .map(eastern_date_time_text)
                .unwrap_or_else(|| "unavailable".to_string()),
            issue.theme
        ));
    }
    let issue_list = |numbers: &[u64]| {
        numbers
            .iter()
            .map(|number| match &sources.github_repo {
                Some(repo) => format!("[#{number}](https://github.com/{repo}/issues/{number})"),
                None => format!("#{number}"),
            })
            .collect::<Vec<_>>()
            .join(", ")
    };
    output.push('\n');
    if sources.issues.is_empty() {
        output.push_str("No GitHub issues are counted in this report's fixes ledger.\n\n");
    }
    if !sources.issues_still_open.is_empty() {
        output.push_str(&format!(
            "Referenced and still open at retrieval, so not counted: {}.\n\n",
            issue_list(&sources.issues_still_open)
        ));
    }
    if !sources.issues_closed_outside.is_empty() {
        output.push_str(&format!(
            "Referenced but closed outside this release's counting window, so not counted: {}.\n\n",
            issue_list(&sources.issues_closed_outside)
        ));
    }
    output.push_str("## Install\n\n");
    output.push_str("```bash\n");
    output.push_str(&format!(
        "cas update --yes --version {version}\n"
    ));
    output.push_str("```\n\n");
    output.push_str("| Asset | Digest | Size |\n| --- | --- | ---: |\n");
    for asset in &sources.assets {
        output.push_str(&format!(
            "| {} | {} | {} |\n",
            escape_table(&asset.name),
            asset.digest.as_deref().unwrap_or("unavailable"),
            asset
                .size
                .map(|size| size.to_string())
                .unwrap_or_else(|| "unavailable".to_string())
        ));
    }
    if sources.assets.is_empty() {
        output.push_str("| No published assets | unavailable | unavailable |\n");
    }
    output.push_str("\n## Evidence and scope\n\n");
    output.push_str(&format!(
        "Retrieved: {}.\n\n",
        eastern_date_time_text(&sources.retrieved_at)
    ));
    output.push_str(&format!("{}\n\n", source_refs));
    output.push_str(
        "Each change is placed in a product area by the words in its title, then its text; a change that matches no area is listed under Other. Correct an area in the Change map table before rendering.\n\n",
    );
    if let Some(pr) = &sources.release_pr {
        output.push_str(&format!(
            "Release PR: {} (merged {}).\n\n",
            pr.url
                .as_deref()
                .map(|url| format!("<{url}>"))
                .unwrap_or_else(|| format!("#{}", pr.number)),
            eastern_date_time(pr.merged_at)
        ));
    }
    if let Some((path, _)) = &sources.release_notes {
        output.push_str(&format!(
            "Release-notes draft: `{}`.\n\n",
            display_path(path, project_root)
        ));
    } else {
        output.push_str("Release-notes draft: unavailable.\n\n");
    }
    if let Some(path) = &sources.changelog_path {
        output.push_str(&format!(
            "CHANGELOG source: `{}`.\n\n",
            display_path(path, project_root)
        ));
    } else {
        output.push_str("CHANGELOG source: unavailable.\n\n");
    }
    if let Some(release) = &sources.release {
        let release_url = release
            .url
            .as_deref()
            .map(|url| format!("<{url}>"))
            .unwrap_or_else(|| "unavailable".to_string());
        output.push_str(&format!(
            "GitHub release: {} ({}).\n\n",
            release_url,
            if release.is_draft == Some(true) {
                "draft"
            } else {
                "published or unknown"
            }
        ));
    } else {
        output.push_str("GitHub release: unavailable.\n\n");
    }
    output.push_str(&receipt_reference_line(
        sources.release_evidence.receipt_reference.as_ref(),
    ));
    // cas-2bef: the renderer fixes the top-level sections, so the stage
    // budgets are a subsection of the evidence.
    output.push_str(&pipeline_latency_section(&sources.release_evidence));
    if sources.warnings.is_empty() {
        output.push_str("No source warnings.\n");
    } else {
        output.push_str("Source warnings:\n\n");
        for warning in &sources.warnings {
            output.push_str(&format!("- {}\n", escape_table(warning)));
        }
    }
    output
}

fn was_now_sections(sources: &AcquiredSources, user_facing: bool) -> String {
    let mut output = String::new();
    let release_note_entries = sources
        .release_note_articles
        .iter()
        .filter(|article| article.user_facing == user_facing)
        .take(80)
        .collect::<Vec<_>>();
    if !release_note_entries.is_empty() {
        let mut grouped = Vec::<(String, Vec<&ReleaseNoteArticle>)>::new();
        for article in release_note_entries {
            let group = article
                .group
                .clone()
                .map(|group| report_group_heading(&group, user_facing))
                .unwrap_or_else(|| "Release changes".to_string());
            if let Some((_, articles)) = grouped.iter_mut().find(|(known, _)| known == &group) {
                articles.push(article);
            } else {
                grouped.push((group, vec![article]));
            }
        }
        for (group, articles) in grouped {
            output.push_str(&format!("### {group}\n\n"));
            for article in articles {
                output.push_str(&format!(
                    "#### {}\n\nWas: {}\n\nNow: {}\n\n",
                    article.title,
                    article.was.trim().trim_end_matches('→').trim_end(),
                    article.now
                ));
            }
        }
        return output;
    }

    let entries = sources
        .changelog
        .as_ref()
        .map(|section| section.entries.as_slice())
        .unwrap_or(&[]);
    let selected = entries
        .iter()
        .filter(|entry| {
            if user_facing {
                entry.category.eq_ignore_ascii_case("Added")
                    || entry.category.eq_ignore_ascii_case("Changed")
                    || entry.category.eq_ignore_ascii_case("Fixed")
            } else {
                true
            }
        })
        .take(80)
        .collect::<Vec<_>>();
    let mut groups: HashMap<&str, Vec<&ChangelogEntry>> = HashMap::new();
    for entry in selected {
        groups
            .entry(entry.category.as_str())
            .or_default()
            .push(entry);
    }
    for category in [
        "Added",
        "Changed",
        "Fixed",
        "Security",
        "Deprecated",
        "Removed",
        "Changes",
    ] {
        let Some(category_entries) = groups.get(category) else {
            continue;
        };
        output.push_str(&format!("### {}\n\n", category));
        for entry in category_entries {
            let now = clean_markdown_text(&entry.text);
            let title = heading_from_entry(&now);
            output.push_str(&format!(
                "#### {}\n\nWas: The prior behavior is not stated in the available release sources.\n\nNow: {}\n\n",
                title, now
            ));
        }
    }
    if output.is_empty() {
        output.push_str("### Source availability\n\n#### Evidence pending\n\nWas: No release change entry was available.\n\nNow: Add the release-notes or CHANGELOG source, then rerun with `--refresh-sources`.\n\n");
    }
    output
}

fn source_reference_line(sources: &AcquiredSources, tag: &str, project_root: &Path) -> String {
    let mut refs = Vec::new();
    if let Some(path) = &sources.changelog_path {
        refs.push(format!("CHANGELOG `{}`", display_path(path, project_root)));
    }
    if let Some((path, _)) = &sources.release_notes {
        refs.push(format!(
            "release-notes draft `{}`",
            display_path(path, project_root)
        ));
    }
    if let Some(repo) = &sources.github_repo {
        refs.push(format!("GitHub `{repo}` release {tag}"));
    }
    if refs.is_empty() {
        "Source: release evidence unavailable; this report remains an honest draft.".to_string()
    } else {
        format!(
            "Source: {}; retrieved {}.",
            refs.join(", "),
            eastern_date_time_text(&sources.retrieved_at)
        )
    }
}

fn report_group_heading(group: &str, user_facing: bool) -> String {
    let group = group.trim().trim_end_matches(':').trim();
    if group.eq_ignore_ascii_case("Only reply") {
        format!("{group} ({})", if user_facing { "user" } else { "dev" })
    } else {
        group.to_string()
    }
}

fn parse_release_note_draft(content: &str) -> ReleaseNoteDraft {
    let mut draft = ReleaseNoteDraft::default();
    let mut audience = None;
    let mut group = None;
    let mut block: Vec<String> = Vec::new();

    let flush = |block: &mut Vec<String>,
                 draft: &mut ReleaseNoteDraft,
                 audience: Option<bool>,
                 group: Option<String>| {
        if block.is_empty() {
            return;
        }
        let text = block.join(" ");
        if is_release_note_top_level(&text) {
            if audience == Some(true) && draft.user_punch.is_none() {
                draft.user_punch = parse_release_note_values(&text).map(|(_, now)| now);
            }
        } else if let Some(article) = parse_release_note_article(&text, audience, group.as_deref())
        {
            draft.articles.push(article);
        }
        block.clear();
    };

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            continue;
        }
        if trimmed.is_empty() {
            flush(&mut block, &mut draft, audience, group.clone());
            continue;
        }
        if let Some(next_audience) = release_note_audience(trimmed) {
            if is_release_note_section_label(trimmed) {
                flush(&mut block, &mut draft, audience, group.clone());
                audience = Some(next_audience);
                group = None;
                continue;
            }
            if is_release_note_top_level(trimmed) {
                flush(&mut block, &mut draft, audience, group.clone());
                audience = Some(next_audience);
            }
        }
        if let Some(next_group) = release_note_group_label(trimmed) {
            flush(&mut block, &mut draft, audience, group.clone());
            group = Some(next_group);
            continue;
        }
        let starts_bullet =
            trimmed.starts_with('•') || trimmed.starts_with("- ") || trimmed.starts_with("* ");
        if starts_bullet && block.iter().any(|line| contains_marker(line, "was:")) {
            flush(&mut block, &mut draft, audience, group.clone());
        }
        block.push(trimmed.to_string());
    }
    flush(&mut block, &mut draft, audience, group);
    draft
}

fn parse_release_note_article(
    text: &str,
    audience: Option<bool>,
    group: Option<&str>,
) -> Option<ReleaseNoteArticle> {
    if is_release_note_top_level(text) {
        return None;
    }
    let user_facing = audience?;
    let (was, now, was_start) = parse_release_note_values_with_offset(text)?;
    Some(ReleaseNoteArticle {
        title: release_note_title(&text[..was_start]),
        was,
        now,
        user_facing,
        group: group.map(ToOwned::to_owned),
    })
}

fn parse_release_note_values(text: &str) -> Option<(String, String)> {
    let (was, now, _) = parse_release_note_values_with_offset(text)?;
    Some((was, now))
}

fn parse_release_note_values_with_offset(text: &str) -> Option<(String, String, usize)> {
    let was_start = find_marker(text, "was:")?;
    let after_was = was_start + "was:".len();
    let now_relative = find_marker(&text[after_was..], "now:")?;
    let now_start = after_was + now_relative;
    let was = clean_release_note_value(&text[after_was..now_start]);
    let now = clean_release_note_value(&text[now_start + "now:".len()..]);
    if was.is_empty() || now.is_empty() {
        return None;
    }
    Some((was, now, was_start))
}

fn release_note_title(value: &str) -> String {
    let mut title = value
        .trim()
        .trim_start_matches(['•', '-', '*'])
        .trim()
        .trim_end_matches('—')
        .trim()
        .trim_matches('*')
        .trim()
        .to_string();
    if title.is_empty() {
        title = "Release change".to_string();
    }
    title
}

fn release_note_group_label(line: &str) -> Option<String> {
    let value = line.trim();
    let value = value.strip_prefix("### ").unwrap_or(value);
    let value = value
        .strip_prefix("**")
        .and_then(|value| value.strip_suffix("**"))
        .or_else(|| {
            value
                .strip_prefix('*')
                .and_then(|value| value.strip_suffix('*'))
        })?
        .trim();
    let normalized = value.trim_end_matches(':').trim();
    if normalized.is_empty()
        || normalized.eq_ignore_ascii_case("Top-level")
        || normalized.eq_ignore_ascii_case("Reply (Was → Now)")
        || normalized.contains("Was:")
        || normalized.contains("Now:")
        || is_release_note_top_level(normalized)
    {
        return None;
    }
    Some(normalized.to_string())
}

fn is_release_note_top_level(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    lower.contains("live on production")
        && (lower.contains("— user —") || lower.contains("— dev —"))
}

fn clean_release_note_value(value: &str) -> String {
    value
        .trim()
        .trim_start_matches('*')
        .trim_end_matches('*')
        .trim()
        .to_string()
}

fn contains_marker(value: &str, marker: &str) -> bool {
    find_marker(value, marker).is_some()
}

fn find_marker(value: &str, marker: &str) -> Option<usize> {
    value.to_ascii_lowercase().find(marker)
}

fn release_note_audience(line: &str) -> Option<bool> {
    let lower = line
        .trim()
        .trim_start_matches('#')
        .trim()
        .trim_matches('*')
        .trim()
        .to_ascii_lowercase();
    if lower.contains("— user —") || lower == "user thread" {
        Some(true)
    } else if lower.contains("— dev —") || lower == "dev thread" {
        Some(false)
    } else if lower.starts_with("user top-level") || lower.starts_with("user reply") {
        Some(true)
    } else if lower.starts_with("dev top-level") || lower.starts_with("dev reply") {
        Some(false)
    } else {
        None
    }
}

fn is_release_note_section_label(line: &str) -> bool {
    let lower = line
        .trim()
        .trim_start_matches('#')
        .trim()
        .to_ascii_lowercase();
    lower == "user thread"
        || lower == "dev thread"
        || lower.starts_with("user top-level")
        || lower.starts_with("user reply")
        || lower.starts_with("dev top-level")
        || lower.starts_with("dev reply")
}

/// The report's "Markdown source" and "Print edition" link targets, given
/// their repository-relative paths. cas-692a: Chromium prints a relative link
/// as a file:// URL into the generating checkout, so with a known GitHub
/// repository both name the files on its default branch: reports are committed after the
/// release tag is created. Without one they stay
/// sibling names; the PDF printer then drops their link targets.
fn report_link_hrefs(
    github_repo: Option<&str>,
    default_branch: &str,
    source: &str,
    pdf: &str,
) -> (String, String) {
    match github_repo {
        Some(repo) => (
            format!("https://github.com/{repo}/blob/{default_branch}/{source}"),
            format!("https://github.com/{repo}/blob/{default_branch}/{pdf}"),
        ),
        None => {
            let name = |path: &str| path.rsplit('/').next().unwrap_or(path).to_string();
            (name(source), name(pdf))
        }
    }
}

fn render_html(
    script: &Path,
    source: &Path,
    output: &Path,
    project_root: &Path,
    timeout: Duration,
    links: (String, String),
) -> anyhow::Result<()> {
    let mut command = Command::new("python3");
    let (source_href, pdf_href) = links;
    command
        .current_dir(project_root)
        .arg(script)
        .arg(source)
        .arg("--output")
        .arg(output)
        .arg("--project-root")
        .arg(project_root)
        .arg("--cas-bin")
        .arg(std::env::current_exe()?)
        .arg("--source-href")
        .arg(source_href)
        .arg("--pdf-href")
        .arg(pdf_href);
    let output_result = run_command(&mut command, Deadline::after(timeout), timeout)
        .map_err(|error| anyhow::anyhow!(bounded_error("python3", error)))?;
    if !output_result.status.success() {
        bail!(
            "release report HTML renderer failed: {}",
            command_output_detail(&output_result)
        );
    }
    Ok(())
}

fn gh_json(project_root: &Path, args: &[&str]) -> anyhow::Result<Value> {
    let binary = std::env::var_os("GH_BIN").unwrap_or_else(|| "gh".into());
    let mut command = Command::new(binary);
    command.current_dir(project_root).args(args);
    let output = run_command(&mut command, Deadline::after(GH_TIMEOUT), GH_TIMEOUT)
        .map_err(|error| anyhow::anyhow!(bounded_error("gh", error)))?;
    if !output.status.success() {
        bail!("{}", command_output_detail(&output));
    }
    serde_json::from_slice(&output.stdout).context("gh returned malformed JSON")
}

fn find_changelog(root: &Path) -> Option<PathBuf> {
    ["CHANGELOG.md", "CHANGELOG.markdown", "CHANGELOG"]
        .into_iter()
        .map(|name| root.join(name))
        .find(|path| path.is_file())
}

fn find_release_notes(root: &Path, version: &str) -> Option<(PathBuf, String)> {
    let directory = root.join("docs/release-notes");
    let mut candidates = Vec::new();
    let wanted = version.to_ascii_lowercase();
    let bare = wanted.trim_start_matches('v');
    collect_files(&directory, &mut candidates);
    candidates.sort();
    candidates.into_iter().find_map(|path| {
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            return None;
        }
        let filename = path.file_name()?.to_string_lossy().to_ascii_lowercase();
        if !filename.contains(&wanted) && !filename.contains(bare) {
            return None;
        }
        fs::read_to_string(&path)
            .ok()
            .map(|content| (path, content))
    })
}

fn collect_files(directory: &Path, output: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, output);
        } else if path.is_file() {
            output.push(path);
        }
    }
}

/// cas-1a83: resolve and check `--latency-receipt` before any source is
/// fetched. The receipt must exist and name this release's tag.
fn standalone_latency_receipt(
    project_root: &Path,
    path: &Path,
    tag: &str,
) -> anyhow::Result<PathBuf> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        project_root.join(path)
    };
    let content = fs::read_to_string(&path)
        .with_context(|| format!("could not read latency receipt {}", path.display()))?;
    let receipt_tag = parse_key_values(&content).get("TAG").cloned();
    if receipt_tag
        .as_deref()
        .map(|value| value.trim_start_matches('v'))
        != Some(tag.trim_start_matches('v'))
    {
        anyhow::bail!(
            "latency receipt {} is for {}, not {tag}",
            path.display(),
            receipt_tag.as_deref().unwrap_or("no TAG")
        );
    }
    Ok(path)
}

fn find_release_evidence(
    tag: &str,
    release: &Option<ReleaseMetadata>,
    latency_receipt: Option<&Path>,
    config: &Config,
) -> ReleaseEvidence {
    let (artifacts_root, root_label) = release_artifacts_root(config);
    find_release_evidence_in(
        tag,
        release,
        &artifacts_root.join("release"),
        latency_receipt.and_then(Path::parent),
        &artifacts_root,
        &root_label,
    )
}

/// cas-0dd1: the `[factory] artifacts_root` that release runs write under,
/// and how the report names it: the configured value as written, or
/// `~/.cas/artifacts` when unset.
fn release_artifacts_root(config: &Config) -> (PathBuf, String) {
    let configured = config.factory().artifacts_root;
    let configured = configured.as_deref().map(str::trim).filter(|value| !value.is_empty());
    (
        crate::config::resolved_factory_artifacts_root(configured),
        configured
            .unwrap_or(crate::config::FACTORY_ARTIFACTS_ROOT_DEFAULT)
            .to_string(),
    )
}

/// Release evidence under an artifacts root laid out as release runs write
/// it, `<root>/release/<run>/`.
#[cfg(test)]
fn find_release_evidence_under(
    tag: &str,
    release: &Option<ReleaseMetadata>,
    artifacts_root: &Path,
) -> ReleaseEvidence {
    find_release_evidence_in(
        tag,
        release,
        &artifacts_root.join("release"),
        None,
        artifacts_root,
        crate::config::FACTORY_ARTIFACTS_ROOT_DEFAULT,
    )
}

/// Files under `root` count when their path names the tag. A standalone
/// run directory is this release's by the checked receipt, so all of its
/// files count; they are read last and win over the shared root.
fn find_release_evidence_in(
    tag: &str,
    release: &Option<ReleaseMetadata>,
    root: &Path,
    run_dir: Option<&Path>,
    artifacts_root: &Path,
    root_label: &str,
) -> ReleaseEvidence {
    let wanted = tag.to_ascii_lowercase();
    let bare = wanted.trim_start_matches('v');
    let mut files = Vec::new();
    collect_files(&root, &mut files);
    files.sort();
    let mut run_files = Vec::new();
    if let Some(dir) = run_dir {
        collect_files(dir, &mut run_files);
        run_files.sort();
    }
    let run_file_count = run_files.len();
    files.extend(run_files);
    let first_run_file = files.len() - run_file_count;
    let mut evidence = ReleaseEvidence {
        stage_trend: stage_trend(&files, tag, 5),
        ..ReleaseEvidence::default()
    };
    let mut green_epoch = None;
    let mut main_landed_epoch = None;
    let mut published_at = release
        .as_ref()
        .and_then(|release| release.published_at.clone());
    for (index, path) in files.into_iter().enumerate() {
        let path_text = path.to_string_lossy().to_ascii_lowercase();
        if index < first_run_file && !path_text.contains(&wanted) && !path_text.contains(bare) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if path.file_name().and_then(|name| name.to_str()) == Some("gate.green.epoch") {
            green_epoch = content.trim().parse::<i64>().ok();
        }
        if path.file_name().and_then(|name| name.to_str()) == Some("main.landed.epoch") {
            main_landed_epoch = content.trim().parse::<i64>().ok();
        }
        let values = parse_key_values(&content);
        if let Some(value) = values.get("GATE_GREEN_EPOCH") {
            green_epoch = value.parse::<i64>().ok();
        }
        if let Some(value) = values.get("MAIN_LANDED_EPOCH") {
            main_landed_epoch = value.parse::<i64>().ok();
        }
        if let Some(value) = values.get("PUBLISHED_AT") {
            published_at = Some(value.clone());
        }
        if values.contains_key("PUBLISH_LATENCY_SECONDS") {
            evidence.record_latency(&values);
        }
        if path.file_name().and_then(|name| name.to_str()) == Some("lesson-recurrence.json") {
            evidence.lesson_recurrence = serde_json::from_str::<LessonRecurrence>(&content)
                .ok()
                .map(|recurrence| recurrence.summary());
        }
        if values.contains_key("PUBLISH_LATENCY_SECONDS") || path_text.contains("published.receipt")
        {
            evidence.receipt_reference = Some(match path.strip_prefix(artifacts_root) {
                Ok(relative) => ReceiptReference::ArtifactsRootRelative {
                    root_label: root_label.to_string(),
                    path: relative.to_string_lossy().replace('\\', "/"),
                },
                Err(_) => ReceiptReference::StandaloneInput,
            });
        }
    }
    if evidence.publication_start_kind.as_deref() == Some("main-landed") {
        // In the fallback case, a local release-train gate file is not
        // merge-queue evidence. Keep this interval explicitly main-based.
        green_epoch = None;
    } else if evidence.publication_start_kind.as_deref() != Some("gate-green") {
        main_landed_epoch = None;
    }
    evidence.tag_published_at = published_at.clone();
    if let (Some(epoch), Some(published)) = (green_epoch, published_at.as_deref()) {
        if let Ok(published) = DateTime::parse_from_rfc3339(published) {
            let seconds = published.timestamp() - epoch;
            if seconds >= 0 {
                evidence.green_to_published_seconds = Some(seconds);
                evidence.green_at =
                    DateTime::<Utc>::from_timestamp(epoch, 0).map(|value| value.to_rfc3339());
            }
        }
    }
    if let (Some(epoch), Some(published)) = (main_landed_epoch, published_at.as_deref()) {
        if let Ok(published) = DateTime::parse_from_rfc3339(published) {
            let seconds = published.timestamp() - epoch;
            if seconds >= 0 {
                evidence.main_to_published_seconds = Some(seconds);
            }
        }
    }
    evidence
}

/// cas-9734: `<run-dir>/lesson-recurrence.json`, written by release-learning.py.
#[derive(Debug, Clone, serde::Deserialize)]
pub(crate) struct LessonRecurrence {
    pub version: String,
    pub rescues: u64,
    pub recurrences: u64,
    pub rate: f64,
    pub classes: Vec<String>,
}

impl LessonRecurrence {
    /// `2 of 4 rescues (50%) repeated a learned lesson: scratch-base, version-literals`
    pub(crate) fn summary(&self) -> String {
        let mut summary = format!(
            "{} of {} rescue{} ({:.0}%) repeated a learned lesson",
            self.recurrences,
            self.rescues,
            if self.rescues == 1 { "" } else { "s" },
            self.rate * 100.0
        );
        if !self.classes.is_empty() {
            summary.push_str(&format!(": {}", self.classes.join(", ")));
        }
        summary
    }
}

/// Every recorded release run's lesson recurrence, newest version first.
pub(crate) fn lesson_recurrences(config: &Config) -> Vec<LessonRecurrence> {
    let (artifacts_root, _) = release_artifacts_root(config);
    let mut runs: Vec<LessonRecurrence> = fs::read_dir(artifacts_root.join("release"))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|run| fs::read_to_string(run.path().join("lesson-recurrence.json")).ok())
        .filter_map(|json| serde_json::from_str(&json).ok())
        .collect();
    let key = |run: &LessonRecurrence| -> Vec<u64> {
        run.version.split('.').map(|part| part.parse().unwrap_or(0)).collect()
    };
    runs.sort_by_key(|run| std::cmp::Reverse(key(run)));
    runs
}

fn parse_key_values(content: &str) -> HashMap<String, String> {
    content
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect()
}

fn extract_issue_numbers(text: &str) -> Vec<u64> {
    // First, find all PR-style references and mark their numbers for exclusion
    let pr_numbers = extract_pr_numbers(text);

    let regex = Regex::new(r"#([0-9]+)\b").expect("issue reference regex");
    regex
        .captures_iter(text)
        .filter_map(|capture| {
            let number_str = capture.get(1)?;
            let number: u64 = number_str.as_str().parse().ok()?;

            // Skip if this number is in the PR list
            if pr_numbers.contains(&number) {
                return None;
            }

            Some(number)
        })
        .collect()
}

fn extract_pr_numbers(text: &str) -> Vec<u64> {
    let mut pr_numbers = Vec::new();
    let text_lower = text.to_lowercase();

    // Match "PR #N, #M..." or "PRs #N, #M..." or "pull request #N, #M..."
    // This regex finds PR/PRs/pull request followed by one or more # numbers separated by commas
    let pattern = Regex::new(r"(?i)(?:^|\s)(?:pr|prs|pull\s+request(?:\(s\))?)\s*(?:#[0-9]+(?:\s*,\s*)?)+")
        .expect("PR context regex");

    for mat in pattern.find_iter(&text_lower) {
        let pr_section = mat.as_str();
        // Extract all numbers from this PR section
        let number_regex = Regex::new(r"#([0-9]+)").expect("number extraction regex");
        for num_match in number_regex.captures_iter(pr_section) {
            if let Some(num_str) = num_match.get(1) {
                if let Ok(num) = num_str.as_str().parse() {
                    pr_numbers.push(num);
                }
            }
        }
    }

    pr_numbers
}

fn extract_issue_url_numbers(text: &str) -> Vec<u64> {
    let regex = Regex::new(r"/issues/([0-9]+)\b").expect("issue URL regex");
    regex
        .captures_iter(text)
        .filter_map(|capture| capture.get(1)?.as_str().parse().ok())
        .collect()
}

fn normalize_version(value: &str) -> anyhow::Result<String> {
    let value = value.trim().trim_start_matches('v');
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_' | b'+'))
        || !value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_digit())
    {
        bail!(
            "invalid release version `{}`; use a safe version such as 3.20.0",
            value
        );
    }
    Ok(value.to_string())
}

fn project_name(config: &Config, root: &Path) -> String {
    config
        .project
        .as_ref()
        .and_then(|project| project.canonical_id.as_deref())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            root.file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("Project")
                .to_string()
        })
}

fn display_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| path.display().to_string())
}

fn write_text(path: &Path, content: &str) -> anyhow::Result<()> {
    fs::write(path, content).with_context(|| format!("could not write {}", path.display()))
}

fn count_issues_in_source(source: &str) -> usize {
    source
        .lines()
        .skip_while(|line| !line.starts_with("## Fixes ledger"))
        .skip(1)
        .take_while(|line| !line.starts_with("## "))
        .filter(|line| line.starts_with("| [#") || line.starts_with("| #"))
        .count()
}

/// The change map's Total in an existing source; 0 for a v1 issue map.
fn count_changes_in_source(source: &str) -> usize {
    let map = source
        .lines()
        .skip_while(|line| !line.starts_with("## Change map"))
        .take_while(|line| !line.starts_with("## Release at a glance"))
        .collect::<Vec<_>>();
    if !map.iter().any(|line| line.starts_with("| Area | Changes |")) {
        return 0;
    }
    map.iter()
        .find_map(|line| line.strip_prefix("| Total | "))
        .and_then(|rest| rest.split('|').next())
        .and_then(|count| count.trim().parse().ok())
        .unwrap_or(0)
}

fn date_from_timestamp(value: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.date_naive().to_string())
        .or_else(|| {
            NaiveDate::parse_from_str(value, "%Y-%m-%d")
                .ok()
                .map(|date| date.to_string())
        })
}

/// The operator reads clock times in US Eastern, 12-hour form ("1:04 AM ET").
/// Eastern daylight time (UTC-4) runs from the second Sunday in March at
/// 2:00 AM EST (07:00 UTC) to the first Sunday in November at 2:00 AM EDT
/// (06:00 UTC); standard time (UTC-5) covers the rest of the year. Machine
/// fields (front matter, JSON, receipts) stay in UTC ISO-8601.
pub(crate) fn eastern(instant: DateTime<Utc>) -> DateTime<FixedOffset> {
    let year = instant.year();
    let nth_sunday = |month: u32, nth: i64| {
        let first = NaiveDate::from_ymd_opt(year, month, 1).expect("valid month start");
        let to_sunday = (7 - first.weekday().num_days_from_sunday() as i64) % 7;
        first + chrono::Duration::days(to_sunday + 7 * (nth - 1))
    };
    let daylight_from = nth_sunday(3, 2).and_hms_opt(7, 0, 0).expect("valid time").and_utc();
    let daylight_until = nth_sunday(11, 1).and_hms_opt(6, 0, 0).expect("valid time").and_utc();
    let hours = if instant >= daylight_from && instant < daylight_until {
        -4
    } else {
        -5
    };
    instant.with_timezone(&FixedOffset::east_opt(hours * 3600).expect("valid offset"))
}

/// `1:04 AM ET`
pub(crate) fn eastern_time(instant: DateTime<Utc>) -> String {
    eastern(instant).format("%-I:%M %p ET").to_string()
}

/// `16 October 2026 · 1:04 AM ET`, dated in Eastern time.
fn eastern_date_time(instant: DateTime<Utc>) -> String {
    format!(
        "{} · {}",
        eastern(instant).format("%-d %B %Y"),
        eastern_time(instant)
    )
}

/// An RFC 3339 timestamp as Eastern date and time; other text is kept.
fn eastern_date_time_text(value: &str) -> String {
    parse_utc(Some(value))
        .map(eastern_date_time)
        .unwrap_or_else(|| value.to_string())
}

fn format_publication_line(value: &str) -> String {
    match parse_utc(Some(value)) {
        Some(published) => format!("Published {}", eastern_date_time(published)),
        None => format!("Published {value}"),
    }
}

fn format_verdict(value: &str) -> String {
    let value = value.trim();
    let mut chars = value.chars();
    let mut verdict = chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>())
        .unwrap_or_default();
    verdict.push_str(chars.as_str());
    if !(verdict.ends_with('.') || verdict.ends_with('!') || verdict.ends_with('?')) {
        verdict.push('.');
    }
    verdict
}

fn format_duration(seconds: i64) -> String {
    if seconds < 60 {
        format!("{seconds}s")
    } else {
        format!("{}m {}s", seconds / 60, seconds % 60)
    }
}

fn clean_markdown_text(text: &str) -> String {
    text.replace("\r", "")
        .replace('|', "&#124;")
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with("<!--") && !line.starts_with("-->") && !line.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn heading_from_entry(text: &str) -> String {
    let text = text.trim_start_matches(['*', '[']);
    let text = text.split_once(':').map(|(head, _)| head).unwrap_or(text);
    let text = text.split_once(" — ").map(|(head, _)| head).unwrap_or(text);
    let mut heading = text.trim().to_string();
    if heading.len() > 100 {
        let mut end = 97;
        while !heading.is_char_boundary(end) {
            end -= 1;
        }
        heading.truncate(end);
        heading.push_str("...");
    }
    if heading.is_empty() {
        "Release change".to_string()
    } else {
        heading
    }
}

fn escape_table(value: &str) -> String {
    value.replace('|', "&#124;").replace('\n', " ")
}

fn command_output_detail(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| line.chars().take(500).collect())
        .unwrap_or_else(|| output.status.to_string())
}

fn bounded_error(command: &str, error: BoundedCommandError) -> String {
    match error {
        BoundedCommandError::TimedOut => format!("{command} timed out"),
        BoundedCommandError::Io => format!("{command} is unavailable or could not be executed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cas-692a: Chromium prints a relative link as a file:// URL into the
    /// generating checkout, so a report names its source and print edition
    /// by their GitHub URLs on main, where the post-release report is committed.
    #[test]
    fn report_links_name_the_main_github_files_not_the_checkout_cas_692a() {
        let (source, pdf) = report_link_hrefs(
            Some("o/r"),
            "main",
            "docs/release-reports/v9.99.1.md",
            "docs/release-reports/v9.99.1.pdf",
        );
        // pin: the exact URL is the link a reader of the published PDF follows.
        assert_eq!(source, "https://github.com/o/r/blob/main/docs/release-reports/v9.99.1.md");
        assert_eq!(pdf, "https://github.com/o/r/blob/main/docs/release-reports/v9.99.1.pdf");
    }

    /// cas-7860: issues are closed by hand minutes after the release PR
    /// merges, so a report generated at merge+10s must still count the
    /// issues that close at merge+30s, and must not recount an issue a
    /// previous release already closed.
    #[test]
    fn release_closures_count_issues_closed_after_generation_starts() {
        let prs = serde_json::json!([
            {"number": 1226, "title": "Release v9.99.1: epic/burn-down-v43",
             "body": "### Fixed (#1225)", "url": "https://github.com/o/r/pull/1226",
             "createdAt": "2026-10-16T03:47:00Z", "mergedAt": "2026-10-16T05:04:14Z",
             "closingIssuesReferences": []},
            {"number": 1227, "title": "Release v9.99.2: epic/burn-down-v44",
             "body": "### Added — batch QA (#1203)\n### Fixed — a (#1204)\n### Fixed — b (#1205)\n### Fixed — c (#1225 follow-up)\n### Changed — d (#1211 in part)",
             "url": "https://github.com/o/r/pull/1227",
             "createdAt": "2026-10-16T08:56:17Z", "mergedAt": "2026-10-16T09:29:58Z",
             "closingIssuesReferences": []}
        ]);
        let pr = select_release_pr(&prs, "9.99.2").expect("release PR");
        assert_eq!(pr.number, 1227);
        let window = ClosureWindow {
            opened: pr.created_at,
            closes: pr.merged_at + closure_window(),
        };
        let issue = |number: u64, state: &str, closed: Option<&str>| {
            serde_json::json!({"number": number, "title": format!("Issue {number}"),
                "url": format!("https://github.com/o/r/issues/{number}"),
                "state": state, "closedAt": closed, "labels": []})
        };
        let merged = pr.merged_at;
        let closed_at = (merged + chrono::Duration::seconds(30))
            .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let before = serde_json::json!([
            issue(1203, "OPEN", None), issue(1204, "OPEN", None), issue(1205, "OPEN", None),
            issue(1211, "OPEN", None), issue(1225, "CLOSED", Some("2026-10-16T05:11:00Z"))
        ]);
        let after = serde_json::json!([
            issue(1203, "CLOSED", Some(&closed_at)), issue(1204, "CLOSED", Some(&closed_at)),
            issue(1205, "CLOSED", Some(&closed_at)), issue(1211, "OPEN", None),
            issue(1225, "CLOSED", Some("2026-10-16T05:11:00Z"))
        ]);
        let clock = std::cell::Cell::new(merged + chrono::Duration::seconds(10));
        let snapshot = gather_closures(
            || {
                Ok(if clock.get() < merged + chrono::Duration::seconds(30) {
                    before.clone()
                } else {
                    after.clone()
                })
            },
            &pr.referenced,
            Some(&window),
            "o/r",
            || clock.get(),
            |pause| clock.set(clock.get() + chrono::Duration::from_std(pause).unwrap()),
        )
        .unwrap();
        let closed = snapshot
            .closed
            .iter()
            .map(|issue| issue.number)
            .collect::<Vec<_>>();
        assert_eq!(closed, vec![1203, 1204, 1205]);
        assert_eq!(snapshot.closed_outside, vec![1225]);
        assert_eq!(snapshot.still_open, vec![1211]);
    }

    /// cas-2bef: a release over a stage budget fails the latency section,
    /// with every stage's earlier releases read from their receipts.
    #[test]
    fn pipeline_latency_section_fails_an_over_budget_stage_with_its_trend() {
        // cas-692a: with no latency receipt there is nothing it "records".
        let absent = pipeline_latency_section(&ReleaseEvidence::default());
        assert!(absent.contains("no latency receipt was found"), "{absent}");
        assert!(!absent.contains("records"), "{absent}");
        let root = tempfile::tempdir().unwrap();
        for (dir, receipt) in [
            (
                "v9.97.1-release",
                "TAG=v9.97.1\nSTAGE_SECS=gate:840,publish:600\n",
            ),
            (
                "v9.98.1-release",
                "TAG=v9.98.1\nSTAGE_SECS=gate:1797,publish:169\n",
            ),
            ("v9.99.1-release", "TAG=v9.99.1\nSTAGE_SECS=gate:300\n"),
            (
                "v9.99.0-release",
                "TAG=v9.99.0\nSTAGE_SECS=gate:3029,publish:1526\n",
            ),
        ] {
            fs::create_dir_all(root.path().join(dir)).unwrap();
            fs::write(
                root.path().join(dir).join("release-latency.receipt"),
                receipt,
            )
            .unwrap();
        }
        let mut files = Vec::new();
        collect_files(root.path(), &mut files);
        let mut evidence = ReleaseEvidence {
            stage_trend: stage_trend(&files, "v9.99.0", 5),
            ..ReleaseEvidence::default()
        };
        evidence.record_latency(&parse_key_values(
            "COMMIT_TO_MAIN=tasks:13,median:302911,worst:cas-462b:319886\n\
             STAGE_SECS=gate:3029,publish:1526,pr-body:0\n\
             STAGE_BUDGETS=gate:900,publish:1200\n\
             BUDGET_BREACHES=gate:3029/900,publish:1526/1200\n",
        ));
        let section = pipeline_latency_section(&evidence);
        assert!(
            section.contains("FAILED: 2 over budget. gate took 50m 29s against a 15m 0s budget; publish took 25m 26s against a 20m 0s budget."),
            "{section}"
        );
        assert!(
            section.contains(
                "- gate: 50m 29s (FAILED, budget 15m 0s); earlier releases, oldest first: v9.97.1 14m 0s · v9.98.1 29m 57s"
            ),
            "{section}"
        );
        assert!(
            section
                .contains("- pr-body: 0s (no budget); earlier releases, oldest first: unavailable"),
            "{section}"
        );
        assert!(
            section.contains("13 tasks, median 5048m 31s, worst cas-462b at 5331m 26s."),
            "{section}"
        );

        evidence.record_latency(&parse_key_values(
            "STAGE_SECS=gate:600\nSTAGE_BUDGETS=gate:900\nBUDGET_BREACHES=none\n",
        ));
        let section = pipeline_latency_section(&evidence);
        assert!(section.contains("Within budget: every measured stage"), "{section}");
        assert!(
            section.contains("- gate: 10m 0s (ok, budget 15m 0s);"),
            "{section}"
        );

        evidence.record_latency(&parse_key_values("PUBLISH_LATENCY_SECONDS=169\n"));
        assert!(pipeline_latency_section(&evidence).contains("Stage timings are unavailable"));
    }

    #[test]
    fn publication_receipt_interventions_preserve_unknown_and_real_counts() {
        let mut evidence = ReleaseEvidence::default();
        for (value, expected) in [
            ("8", Some(8)),
            ("0", Some(0)),
            ("-1", None),
            ("", None),
            ("unknown", None),
        ] {
            evidence.record_latency(&parse_key_values(&format!("INTERVENTIONS={value}\n")));
            assert_eq!(evidence.interventions, expected);
        }
        evidence.record_latency(&HashMap::new());
        assert_eq!(evidence.interventions, None);
    }

    /// cas-a629: open blockers, a request time that fell back to the cut,
    /// and a pre-event run with a count but no costs.
    #[test]
    fn end_to_end_release_metrics_render_each_blocker_with_its_cost() {
        let mut evidence = ReleaseEvidence::default();
        evidence.record_latency(&parse_key_values(
            "REQUEST_TO_PUBLISHED_SECS=3600\nREQUEST_SOURCE=cut-start\nCUT_TO_PUBLISHED_SECS=3600\n\
             BLOCKER_COUNT=3\nBLOCKER_COSTS=gate:600,pipeline:1700,receipts:100+\n",
        ));
        assert_eq!(
            evidence.request_timing(),
            "60m 0s — from the first cut; no request time recorded"
        );
        assert_eq!(
            evidence.blocker_summary(),
            "3 — 40m 0s blocked: gate 10m 0s; pipeline 28m 20s; receipts 1m 40s (unresolved at publication)"
        );
        evidence.record_latency(&parse_key_values(
            "BLOCKER_COUNT=2\nBLOCKER_COSTS=unavailable\nCUT_TO_PUBLISHED_SECS=unavailable\n",
        ));
        assert_eq!(evidence.blocker_summary(), "2 — time cost unavailable");
        assert_eq!(evidence.cut_to_published_seconds, None);
        assert_eq!(evidence.request_timing(), "unavailable");
        evidence.record_latency(&parse_key_values("BLOCKER_COUNT=0\nBLOCKER_COSTS=none\n"));
        assert_eq!(evidence.blocker_summary(), "0");
        evidence.record_latency(&HashMap::new());
        assert_eq!(evidence.blocker_summary(), "unavailable");
    }

    #[test]
    fn publication_receipt_overrun_remains_visible_in_report_timing() {
        let mut evidence = ReleaseEvidence::default();
        evidence.record_latency(&parse_key_values(
            "PUBLISH_LATENCY_SECONDS=908\nBUDGET_SECONDS=600\nWITHIN_BUDGET=false\n",
        ));
        assert_eq!(
            evidence.publication_timing(),
            "15m 8s — over budget (10m 0s budget)"
        );
        evidence.record_latency(&parse_key_values(
            "PUBLISH_LATENCY_SECONDS=250\nBUDGET_SECONDS=600\nWITHIN_BUDGET=true\n",
        ));
        assert_eq!(
            evidence.publication_timing(),
            "4m 10s — within budget (10m 0s budget)"
        );
        evidence.record_latency(&parse_key_values(
            "PUBLISH_LATENCY_SECONDS=908\nBUDGET_SECONDS=600\nWITHIN_BUDGET=true\n",
        ));
        assert_eq!(evidence.publication_timing(), "15m 8s — budget unavailable");
        evidence.record_latency(&parse_key_values("PUBLISH_LATENCY_SECONDS=-1\n"));
        assert_eq!(evidence.publication_timing(), "unavailable");
        evidence.record_latency(&parse_key_values(
            "PUBLISH_START_KIND=main-landed\nPUBLISH_LATENCY_SECONDS=616\nBUDGET_SECONDS=600\nWITHIN_BUDGET=false\n",
        ));
        assert_eq!(
            evidence.publication_endpoint(),
            Some((
                "Main to published",
                "Release PR merge time to publication when merge-queue green evidence is unavailable; measured budget result"
            ))
        );
    }

    /// cas-0dd1: with `[factory] artifacts_root` configured, the release
    /// receipt is found under that root and named relative to it.
    #[test]
    fn release_receipt_is_found_and_named_under_the_configured_artifacts_root_cas_0dd1() {
        let artifacts = tempfile::tempdir().unwrap();
        let run = artifacts.path().join("release/v9.1.0-latency");
        fs::create_dir_all(&run).unwrap();
        fs::write(
            run.join("release-latency.receipt"),
            "TAG=v9.1.0\nPUBLISH_START_KIND=gate-green\nPUBLISH_LATENCY_SECONDS=300\nBUDGET_SECONDS=600\nWITHIN_BUDGET=true\nGATE_GREEN_EPOCH=1792079228\nPUBLISHED_AT=2026-10-15T15:52:08Z\n",
        )
        .unwrap();
        let configured = artifacts.path().display().to_string();
        let mut config = Config::default();
        let mut factory = config.factory();
        factory.artifacts_root = Some(configured.clone());
        config.factory = Some(factory);

        let evidence = find_release_evidence("v9.1.0", &None, None, &config);

        assert_eq!(evidence.green_to_published_seconds, Some(300));
        assert_eq!(
            receipt_reference_line(evidence.receipt_reference.as_ref()),
            format!(
                "Release receipt (relative to `{configured}`): `release/v9.1.0-latency/release-latency.receipt`.\n\n"
            )
        );
    }

    #[test]
    fn main_push_receipts_report_green_or_main_fallback_without_unavailable_warning() {
        let green_root = tempfile::tempdir().unwrap();
        let green_dir = green_root.path().join("release/v3.55.0-latency");
        fs::create_dir_all(&green_dir).unwrap();
        fs::write(
            green_dir.join("release-latency.receipt"),
            "TAG=v3.55.0\nPUBLISH_START_KIND=gate-green\nPUBLISH_LATENCY_SECONDS=626\nBUDGET_SECONDS=600\nWITHIN_BUDGET=false\nGATE_GREEN_EPOCH=1792079228\nMAIN_LANDED_EPOCH=1792079238\nPUBLISHED_AT=2026-10-15T15:57:34Z\n",
        )
        .unwrap();
        let green = find_release_evidence_under("v3.55.0", &None, green_root.path());
        assert_eq!(green.green_to_published_seconds, Some(626));
        assert_eq!(green.main_to_published_seconds, Some(616));
        assert!(green.has_reportable_publication_timing());
        assert!(green.publication_endpoint().is_none());
        assert_eq!(
            green.publication_timing(),
            "10m 26s — over budget (10m 0s budget)"
        );

        let fallback_root = tempfile::tempdir().unwrap();
        let fallback_dir = fallback_root.path().join("release/v3.55.0-latency");
        fs::create_dir_all(&fallback_dir).unwrap();
        fs::write(fallback_dir.join("gate.green.epoch"), "1792079228\n").unwrap();
        fs::write(fallback_dir.join("main.landed.epoch"), "1792079238\n").unwrap();
        fs::write(
            fallback_dir.join("release-latency.receipt"),
            "TAG=v3.55.0\nPUBLISH_START_KIND=main-landed\nPUBLISH_LATENCY_SECONDS=616\nBUDGET_SECONDS=600\nWITHIN_BUDGET=false\nMAIN_LANDED_EPOCH=1792079238\nPUBLISHED_AT=2026-10-15T15:57:34Z\n",
        )
        .unwrap();
        let fallback = find_release_evidence_under("v3.55.0", &None, fallback_root.path());
        assert_eq!(fallback.green_to_published_seconds, None);
        assert_eq!(fallback.main_to_published_seconds, Some(616));
        assert!(fallback.has_reportable_publication_timing());
        assert_eq!(
            fallback.publication_endpoint().map(|(label, _)| label),
            Some("Main to published")
        );
        assert_eq!(
            ReleaseEvidence::default().has_reportable_publication_timing(),
            false
        );
    }

    #[test]
    fn changelog_parser_selects_keep_a_changelog_section_and_entries() {
        let fixture = "# Changelog\n\n## [2.4.0] - 2026-09-01\n\n### Added\n- New thing (#12)\n\n### Fixed\n- Small fix\n\n## [2.3.0] - 2026-08-01\n- Older\n";
        let section = parse_changelog_section(fixture, "v2.4.0").expect("section");
        assert_eq!(section.date.as_deref(), Some("2026-09-01"));
        assert_eq!(section.entries.len(), 2);
        assert_eq!(section.entries[0].category, "Added");
        assert_eq!(section.entries[1].category, "Fixed");
        assert!(section.body.contains("New thing"));
    }

    #[test]
    fn changelog_summarized_356_headings_count_changed_and_fixed_entries() {
        // These entries use the actual 3.56-style CHANGELOG summary headings.
        let fixture = "# Changelog\n\n## [9.99.1] - 2026-10-15\n\n### Fixed — the factory daemon loop no longer stalls on reminder probes (#1208, cas-a222)\n- External reminder probes (`branch_contained_in`, `tag_exists`, `ci_completed`) run off the daemon loop.\n\n### Changed — coordination and tools (#1188, #1197, #1207, cas-6853, cas-fa1c, cas-48e5, cas-b58a)\n- One `remind` refusal lists every missing requirement with an example.\n\n## [9.99.2] - 2026-10-15\n";
        let changelog = parse_changelog_section(fixture, "9.99.1").expect("section");
        assert_eq!(changelog.entries[0].category, "Fixed");
        assert_eq!(changelog.entries[1].category, "Changed");

        let sources = AcquiredSources {
            project: "fixture".to_string(),
            changes: changelog_changes(&changelog.body),
            release_pr: None,
            closure_window: None,
            issues_still_open: Vec::new(),
            issues_closed_outside: Vec::new(),
            changelog: Some(changelog),
            changelog_path: None,
            release_notes: None,
            release_note_articles: Vec::new(),
            release_note_user_punch: None,
            release: None,
            issues: Vec::new(),
            assets: Vec::new(),
            release_evidence: ReleaseEvidence::default(),
            github_repo: None,
            issue_count: 0,
            warnings: Vec::new(),
            retrieved_at: "2026-10-15T00:00:00Z".to_string(),
        };
        let report = assemble_markdown(Path::new("."), "9.99.1", "v9.99.1", &sources);
        assert!(
            report.contains("| Changes shipped | 2 | 1 fixed, 1 changed; tests and docs not counted |"),
            "{report}"
        );
        assert!(report.contains(
            "| Sessions | 1 | The factory daemon loop no longer stalls on reminder probes |"
        ));
        // Zero and unmeasured values stay off page one, named in the note.
        let page_one = &report[..report.find("## What you can do now").unwrap()];
        assert!(!page_one.contains("| 0 |"), "{page_one}");
        assert!(!page_one.contains("unavailable |"), "{page_one}");
        assert!(report.contains("No GitHub issues are counted in this report"));
        assert!(!report.contains("No GitHub issues were closed with this release."));
        assert!(report.contains("Not measured for this release: green to published"));
    }

    #[test]
    fn changelog_parser_joins_wrapped_entries_and_preserves_inline_code_heading() {
        let fixture = "# Changelog\n\n## [2.4.0] - 2026-09-01\n\n### Changed\n- `cas release report` assembles the\n  complete report from project sources.\n\n### Fixed\n- The report reports supervisor\n  delivery evidence without truncation.\n";
        let section = parse_changelog_section(fixture, "2.4.0").expect("section");

        assert_eq!(section.entries.len(), 2);
        assert_eq!(
            section.entries[0].text,
            "`cas release report` assembles the complete report from project sources."
        );
        assert_eq!(
            section.entries[1].text,
            "The report reports supervisor delivery evidence without truncation."
        );
        assert!(heading_from_entry(&section.entries[0].text).starts_with('`'));
    }

    #[test]
    fn long_unicode_entry_heading_keeps_complete_chars_and_ellipsis_cas_8007() {
        let prefix = "a".repeat(96);
        for character in ["é", "—", "🌟"] {
            // Byte offset 97 lies inside this character, beyond the ASCII prefix.
            let entry = format!("{prefix}{character}{}", "z".repeat(12));
            assert_eq!(heading_from_entry(&entry), format!("{prefix}..."));
        }
        let short = "é".repeat(50);
        assert_eq!(heading_from_entry(&short), short);
        assert_eq!(heading_from_entry(&"a".repeat(101)), format!("{}...", "a".repeat(97)));
    }

    #[test]
    fn release_note_articles_are_partitioned_into_user_and_developer_sections() {
        let draft = r#"User top-level

```text
*Live on production — User — Cassy v2.4.0*
Was: users had to assemble reports by hand. Now: one command builds the report.
```

User reply

```text
• *Readable report* — Was: the old report hid the evidence. Now: the report shows it.
```

Dev top-level

```text
*Live on production — Dev — Cassy v2.4.0*
Was: report sources were gathered manually. Now: the assembler gathers them.
```

Dev reply

```text
• *Assembler detail* — Was: the report omitted its inputs. Now: the assembler records them.
```
"#;
        let parsed = parse_release_note_draft(draft);
        let articles = parsed.articles;
        assert_eq!(
            parsed.user_punch.as_deref(),
            Some("one command builds the report.")
        );
        assert_eq!(articles.len(), 2);
        assert_eq!(
            articles
                .iter()
                .filter(|article| article.user_facing)
                .count(),
            1
        );
        assert_eq!(
            articles
                .iter()
                .filter(|article| !article.user_facing)
                .count(),
            1
        );
        assert_eq!(articles[0].was, "the old report hid the evidence.");
        assert_eq!(articles[0].title, "Readable report");
        assert_eq!(articles[1].now, "the assembler records them.");
        assert_eq!(articles[1].title, "Assembler detail");
        assert_eq!(articles[0].group, None);

        let mut sources = AcquiredSources {
            project: "fixture".to_string(),
            changes: Vec::new(),
            release_pr: None,
            closure_window: None,
            issues_still_open: Vec::new(),
            issues_closed_outside: Vec::new(),
            changelog: None,
            changelog_path: None,
            release_notes: None,
            release_note_articles: articles,
            release_note_user_punch: None,
            release: None,
            issues: Vec::new(),
            assets: Vec::new(),
            release_evidence: ReleaseEvidence::default(),
            github_repo: None,
            issue_count: 0,
            warnings: Vec::new(),
            retrieved_at: "2026-09-09T00:00:00Z".to_string(),
        };
        sources.release_evidence.record_latency(&parse_key_values(
            "PUBLISH_LATENCY_SECONDS=908\nBUDGET_SECONDS=600\nWITHIN_BUDGET=false\nINTERVENTIONS=8\n\
             REQUEST_TO_PUBLISHED_SECS=7200\nREQUEST_SOURCE=recorded\nCUT_TO_PUBLISHED_SECS=3600\n\
             BLOCKER_COUNT=2\nBLOCKER_COSTS=gate:600,pipeline:1700\nBLOCKED_SECS=2300\n",
        ));
        let report = assemble_markdown(Path::new("."), "2.4.0", "v2.4.0", &sources);
        assert!(report.contains("| Tag to published | 15m 8s | Over budget (10m 0s budget);"));
        assert!(report.contains("| Manual interventions | 8 |"));
        // cas-a629: the whole wall clock and each blocker's cost.
        assert!(
            report.contains("| Request to published | 120m 0s |"),
            "{report}"
        );
        assert!(report.contains("| Cut to published | 60m 0s |"), "{report}");
        sources.release_evidence.record_latency(&parse_key_values(
            "PUBLISH_START_KIND=gate-green\nPUBLISH_LATENCY_SECONDS=626\nBUDGET_SECONDS=600\nWITHIN_BUDGET=false\n",
        ));
        sources.release_evidence.green_to_published_seconds = Some(626);
        let green_report = assemble_markdown(Path::new("."), "2.4.0", "v2.4.0", &sources);
        assert!(
            green_report.contains("| Green to published | 10m 26s | Over budget (10m 0s budget);"),
            "{green_report}"
        );
        assert!(
            !green_report.contains("| Tag to published |"),
            "{green_report}"
        );
        sources.release_evidence.record_latency(&parse_key_values(
            "PUBLISH_START_KIND=main-landed\nPUBLISH_LATENCY_SECONDS=616\nBUDGET_SECONDS=600\nWITHIN_BUDGET=false\n",
        ));
        sources.release_evidence.green_to_published_seconds = None;
        sources.release_evidence.main_to_published_seconds = Some(616);
        let main_landed_report = assemble_markdown(Path::new("."), "2.4.0", "v2.4.0", &sources);
        assert!(
            !main_landed_report.contains("| Green to published |"),
            "{main_landed_report}"
        );
        assert!(
            main_landed_report.contains("Not measured for this release: green to published"),
            "{main_landed_report}"
        );
        assert!(
            main_landed_report
                .contains("| Main to published | 10m 16s | Over budget (10m 0s budget);"),
            "{main_landed_report}"
        );
        assert!(
            report.contains("| Blockers | 2 | 38m 20s blocked: gate 10m 0s; pipeline 28m 20s;"),
            "{report}"
        );
        // cas-9734: the release's lesson recurrence rate.
        assert!(!report.contains("| Lesson recurrence |"), "{report}");
        assert!(report.contains("lesson recurrence."), "{report}");
        let recorded: LessonRecurrence = serde_json::from_str(
            r#"{"version":"9.98.0","rescues":4,"recurrences":2,"rate":0.5,"classes":["scratch-base","version-literals"]}"#,
        )
        .unwrap();
        sources.release_evidence.lesson_recurrence = Some(recorded.summary());
        let measured = assemble_markdown(Path::new("."), "2.4.0", "v2.4.0", &sources);
        assert!(
            measured.contains(
                "| Lesson recurrence | 2 of 4 rescues | (50%) repeated a learned lesson: scratch-base, version-literals;"
            ),
            "{measured}"
        );
        sources.release_evidence.lesson_recurrence = None;
        sources.release_evidence.record_latency(&HashMap::new());
        let unavailable = assemble_markdown(Path::new("."), "2.4.0", "v2.4.0", &sources);
        assert!(!unavailable.contains("| Manual interventions |"));
        assert!(unavailable.contains("manual interventions."), "{unavailable}");
        let user = was_now_sections(&sources, true);
        let dev = was_now_sections(&sources, false);
        assert!(user.contains("Readable report"));
        assert!(!user.contains("report sources were gathered manually"));
        assert!(!dev.contains("report sources were gathered manually"));
        assert!(dev.contains("Assembler detail"));
        assert!(!dev.contains("Readable report"));
    }

    #[test]
    fn release_note_groups_drive_verdict_and_titles() {
        let draft = r#"User top-level

```text
*Live on production — User — Cassy v2.4.0*
Was: the old release. Now: the release is ready to inspect.
```

User reply

```text
*Release reports*

• *Build the report* — Was: reports were manual. Now: one command builds them.

*Already live on hosts that updated from main*

• *Recall memory* — Was: recall could disappear. Now: memory stays visible.

• *Fair task closes* — Was: an old task could block a close. Now: verification checks the task's own range (#767).

• *Honest start-up line* — Was: worker identity was unclear. Now: the provider is named.
```

Dev reply

```text
*Release report command*

• *Assembler* — Was: sources were manual. Now: release reports are assembled.
```
"#;
        let parsed = parse_release_note_draft(draft);
        assert_eq!(
            parsed.user_punch.as_deref(),
            Some("the release is ready to inspect.")
        );
        assert_eq!(parsed.articles[0].group.as_deref(), Some("Release reports"));
        assert_eq!(
            parsed.articles[1].group.as_deref(),
            Some("Already live on hosts that updated from main")
        );
        assert_eq!(parsed.articles[2].title, "Fair task closes");

        assert_eq!(
            format_publication_line("2026-09-09T16:51:40Z"),
            "Published 9 September 2026 · 12:51 PM ET"
        );
    }

    #[test]
    fn reply_was_now_headings_do_not_become_release_groups() {
        let draft = r#"## User thread

**Top-level:**

```text
*Live on production — User — Cassy v9.99.1*
Was: release notes had structural headings. Now: report headings describe changes.
```

**Reply (Was → Now):**

```text
• *Wait for results* — Was: checks could finish later. Now: the change waits for checks.
```

## Dev thread

**Top-level:**

```text
*Live on production — Dev — Cassy v9.99.1*
Was: reports lacked evidence. Now: reports include verification evidence.
```

**Reply (Was → Now):**

```text
• *Merge admission* — Was: pending checks were advisory. Now: merges wait for them.
```
"#;
        let parsed = parse_release_note_draft(draft);
        let sources = AcquiredSources {
            project: "fixture".to_string(),
            changes: Vec::new(),
            release_pr: None,
            closure_window: None,
            issues_still_open: Vec::new(),
            issues_closed_outside: Vec::new(),
            changelog: None,
            changelog_path: None,
            release_notes: Some((PathBuf::from("release-notes.md"), draft.to_string())),
            release_note_articles: parsed.articles,
            release_note_user_punch: parsed.user_punch,
            release: None,
            issues: Vec::new(),
            assets: Vec::new(),
            release_evidence: ReleaseEvidence::default(),
            github_repo: None,
            issue_count: 0,
            warnings: Vec::new(),
            retrieved_at: "2026-10-15T00:00:00Z".to_string(),
        };
        let user = was_now_sections(&sources, true);
        let dev = was_now_sections(&sources, false);
        assert!(!user.contains("### Reply (Was → Now)"), "{user}");
        assert!(!dev.contains("### Reply (Was → Now)"), "{dev}");
        assert!(!user.contains("### Top-level"), "{user}");
        assert!(!dev.contains("### Top-level"), "{dev}");
        assert!(user.contains("### Release changes"), "{user}");
        assert!(dev.contains("### Release changes"), "{dev}");
    }

    #[test]
    fn release_pr_body_issue_references_are_collected() {
        let value = serde_json::json!([
            {"body": "Release closes #767", "closingIssuesReferences": []}
        ]);
        let mut referenced = BTreeSet::new();
        collect_pr_issue_references(&value, &mut referenced);
        assert_eq!(referenced.into_iter().collect::<Vec<_>>(), vec![767]);
    }

    #[test]
    fn project_name_prefers_the_configured_canonical_id() {
        let config = Config {
            project: Some(crate::config::ProjectConfig {
                canonical_id: Some("configured-project".to_string()),
                aliases: Vec::new(),
            }),
            ..Config::default()
        };
        assert_eq!(
            project_name(&config, Path::new("/repo/worker-worktree")),
            "configured-project"
        );
    }

    #[test]
    fn issue_fixture_parses_labels_and_assets_preserve_digests() {
        let issues = serde_json::json!([
            {"number": 12, "title": "Build worker", "url": "https://github.com/o/r/issues/12", "state": "CLOSED", "closedAt": "2026-09-01T01:02:03Z", "labels": [{"name": "factory"}], "body": "worker"},
            {"number": 13, "title": "Mysterious change", "url": "https://github.com/o/r/issues/13", "state": "CLOSED", "closedAt": null, "labels": []}
        ]);
        let references = [12_u64, 13].into_iter().collect();
        let parsed = parse_issues(&issues, &references, "o/r");
        assert_eq!(classify_area(&parsed[0].title, ""), "Sessions");
        assert_eq!(classify_area(&parsed[1].title, ""), OTHER_AREA);

        let release = parse_release_metadata(
            &serde_json::json!({
                "publishedAt": "2026-09-01T01:02:03Z",
                "assets": [{"name": "cas.tar.gz", "size": 42, "url": "https://example/a", "digest": "sha256:abc", "contentType": "application/gzip"}]
            }),
            "o/r",
        );
        assert_eq!(release.assets[0].digest.as_deref(), Some("sha256:abc"));
        assert_eq!(release.assets[0].size, Some(42));
    }

    #[test]
    fn version_parser_rejects_path_traversal_and_normalizes_tag() {
        assert_eq!(normalize_version("v3.20.0").unwrap(), "3.20.0");
        assert!(normalize_version("../secret").is_err());
        assert!(normalize_version("v").is_err());
    }

    /// cas-5389: human-facing times are Eastern 12-hour, EDT in October and
    /// EST in January, switching at 2:00 AM local on the US DST Sundays.
    #[test]
    fn eastern_times_follow_us_daylight_saving() {
        let at = |value: &str| parse_utc(Some(value)).expect("timestamp");
        assert_eq!(eastern_time(at("2026-10-16T05:04:14Z")), "1:04 AM ET");
        assert_eq!(eastern_time(at("2026-01-15T17:30:00Z")), "12:30 PM ET");
        assert_eq!(
            eastern_date_time(at("2027-01-05T03:15:00Z")),
            "4 January 2027 · 10:15 PM ET"
        );
        // The Eastern date can differ from the UTC date.
        assert_eq!(
            format_publication_line("2026-10-15T01:34:05Z"),
            "Published 14 October 2026 · 9:34 PM ET"
        );
        // 8 March 2026: 1:59 AM EST, then 3:00 AM EDT.
        assert_eq!(eastern_time(at("2026-03-08T06:59:00Z")), "1:59 AM ET");
        assert_eq!(eastern_time(at("2026-03-08T07:00:00Z")), "3:00 AM ET");
        // 1 November 2026: 1:59 AM EDT, then 1:00 AM EST.
        assert_eq!(eastern_time(at("2026-11-01T05:59:00Z")), "1:59 AM ET");
        assert_eq!(eastern_time(at("2026-11-01T06:00:00Z")), "1:00 AM ET");
        // Non-timestamp text passes through unchanged.
        assert_eq!(eastern_date_time_text("unavailable"), "unavailable");
    }

    /// cas-7860: page one counts shipped changes by area, in plain titles,
    /// and never lists an area without a change.
    #[test]
    fn changelog_changes_group_by_area_without_zero_rows() {
        let section = parse_changelog_section(
            "## [9.99.2] - 2026-10-16\n\n\
             ### Added — a batch QA mode: one staging walk covers every task in a batch (#1203, cas-d697)\n\n\
             - With `qa.mode = \"batch\"`, one walk covers the batch.\n\n\
             ### Fixed — Commander renders its designed type on every machine (cas-4427)\n\n\
             - Commander bundles its fonts.\n\n\
             ### Fixed — after /clear the heartbeat stays with the new session (cas-9735)\n\n\
             - The new session keeps it.\n\n\
             ### Fixed — `cas serve` starts while another agent is creating a task (cas-30e3)\n\n\
             - Boot skips a retired intent.\n\n\
             ### Tests and docs\n\n\
             - Flaky daemon tests are deterministic (cas-a8dc).\n",
            "9.99.2",
        )
        .expect("section");
        let changes = changelog_changes(&section.body);
        assert_eq!(changes.len(), 4, "tests and docs are not counted: {changes:?}");
        assert_eq!(
            changes[0].title,
            "A batch QA mode: one staging walk covers every task in a batch"
        );
        assert!(changes.iter().all(|change| !change.title.contains("cas-")));
        let areas = area_counts(&changes);
        assert_eq!(
            areas
                .iter()
                .map(|area| (area.area.as_str(), area.changes))
                .collect::<Vec<_>>(),
            vec![("Commander", 1), ("Sessions", 2), ("Tasks", 1)]
        );
        assert!(areas.iter().all(|area| area.changes > 0));

        // Untitled headings: each bullet is a change, titled by its bold lead-in.
        let section = parse_changelog_section(
            "## [9.99.3] - 2026-10-16\n\n### Fixed\n\n\
             - **Typing during cleanup** — Was: slow. → Now: off the input loop.\n\
             - **Loaded-proof receipts** — Was: a number could be borrowed. → Now: fields own them.\n\n\
             ### Added\n\n- **Peer ownership board** — Was: little context. → Now: owners are shown.\n\n\
             ### Documentation\n\n- **9.99.2 release report** — committed.\n",
            "9.99.3",
        )
        .expect("section");
        let titles = changelog_changes(&section.body)
            .into_iter()
            .map(|change| change.title)
            .collect::<Vec<_>>();
        assert_eq!(
            titles,
            vec!["Typing during cleanup", "Loaded-proof receipts", "Peer ownership board"]
        );
    }

    #[test]
    fn human_output_wraps_paths_and_remedies_to_the_terminal_width() {
        let result = ReportResult {
            version: "3.19.0".to_string(),
            tag: "v3.19.0".to_string(),
            project: "fixture".to_string(),
            source_path: "/home/pippenz/.cas/artifacts/cas-7dfa/reports/v3.19.0.md".to_string(),
            html_path: "/home/pippenz/.cas/artifacts/cas-7dfa/reports/v3.19.0.html".to_string(),
            pdf_path: Some(
                "/home/pippenz/.cas/artifacts/cas-7dfa/reports/v3.19.0.pdf".to_string(),
            ),
            source_written: false,
            html_written: true,
            pdf_written: true,
            issue_count: 2,
            asset_count: 1,
            change_count: 0,
            change_areas: Vec::new(),
            warnings: vec![
                "source preserved: /home/pippenz/.cas/artifacts/cas-7dfa/reports/v3.19.0.md is authoritative for v3.19.0".to_string(),
            ],
            retrieved_at: "2026-09-09T15:00:00Z".to_string(),
            github_repo: None,
            release_published_at: None,
            green_to_published_seconds: None,
            brand_source: "defaults".to_string(),
            design: None,
        };
        let mut output = Vec::new();
        {
            let mut fmt = crate::ui::components::Formatter::new(
                &mut output,
                crate::ui::components::OutputMode::Plain,
                crate::ui::theme::ActiveTheme::default(),
                80,
            );
            render_human_output(&mut fmt, &result).unwrap();
        }
        let output = String::from_utf8(output).unwrap();
        assert!(
            output.lines().all(|line| line.chars().count() <= 80),
            "human output overflowed:\n{output}"
        );
        assert!(output.contains("  Source: "));
        assert!(output.contains("  HTML:  "));
        assert!(output.contains("  PDF:   "));
        assert!(output.contains("  Remedy: "));
        assert!(
            output.lines().any(|line| line.starts_with("          ")),
            "expected indented continuation line:\n{output}"
        );
    }

    #[test]
    fn human_output_uses_ascii_marks_for_c_locale() {
        let mut env = crate::test_support::TestEnvGuard::new();
        env.set("LC_ALL", "C");
        env.remove("LC_CTYPE");
        env.remove("LANG");
        let result = ReportResult {
            version: "3.19.0".to_string(),
            tag: "v3.19.0".to_string(),
            project: "fixture".to_string(),
            source_path: "docs/release-reports/v3.19.0.md".to_string(),
            html_path: "docs/release-reports/v3.19.0.html".to_string(),
            pdf_path: None,
            source_written: false,
            html_written: true,
            pdf_written: false,
            issue_count: 0,
            asset_count: 0,
            change_count: 0,
            change_areas: Vec::new(),
            warnings: vec!["source preserved: docs/release-reports/v3.19.0.md".to_string()],
            retrieved_at: "2026-09-09T15:00:00Z".to_string(),
            github_repo: None,
            release_published_at: None,
            green_to_published_seconds: None,
            brand_source: "defaults".to_string(),
            design: None,
        };
        let mut output = Vec::new();
        {
            let mut fmt = crate::ui::components::Formatter::new(
                &mut output,
                crate::ui::components::OutputMode::Styled,
                crate::ui::theme::ActiveTheme::default(),
                80,
            );
            render_human_output(&mut fmt, &result).unwrap();
        }
        let output = String::from_utf8(output).unwrap();
        let output = crate::ui::components::test_helpers::strip_ansi_codes(&output);
        assert!(output.contains("[WARN] report ready - v3.19.0"), "{output}");
        assert!(output.contains("  [WARN] source preserved"), "{output}");
        assert!(!output.contains('⚠'));
        assert!(!output.contains('·'));
    }

    #[test]
    fn extract_issue_numbers_excludes_pull_request_references() {
        // PR references should be excluded
        let text_with_prs = "Merged PR #788 and PR #789 to address issues #790, #791";
        let issues = extract_issue_numbers(text_with_prs);
        assert_eq!(issues, vec![790, 791], "Should skip PR #788 and PR #789");

        // PRs (plural) should be excluded
        let text_with_prs_plural = "PRs #792, #793 were merged; closes #794";
        let issues = extract_issue_numbers(text_with_prs_plural);
        assert_eq!(issues, vec![794], "Should skip PRs #792 and #793");

        // pull request should be excluded (case-insensitive)
        let text_with_pull_request = "Pull request #795 fixes issues #796, #797";
        let issues = extract_issue_numbers(text_with_pull_request);
        assert_eq!(issues, vec![796, 797], "Should skip pull request #795");

        // Bare issue references should be kept
        let text_bare = "This fixes #800 and relates to #801";
        let issues = extract_issue_numbers(text_bare);
        assert_eq!(issues, vec![800, 801], "Should keep bare issue references");

        // Issue references in parentheses should be kept
        let text_paren = "Change (#802) was requested in #803";
        let issues = extract_issue_numbers(text_paren);
        assert_eq!(issues, vec![802, 803], "Should keep parenthesized issue references");

        // Real v3.22.1 release notes example
        let v3_22_1_dev_thread = "• Dev thread — PR #788, #789, #790, #791, #792, #794 deliver v3.22.1";
        let issues = extract_issue_numbers(v3_22_1_dev_thread);
        assert_eq!(issues, Vec::<u64>::new(), "Should skip all PRs in the release notes dev thread");

        // Mix of real issues and PR references
        let mixed = "closes #767; Merged via PR #786. Related: PR #788, #789, #790, #791, #792, #794";
        let issues = extract_issue_numbers(mixed);
        assert_eq!(
            issues,
            vec![767],
            "Should only extract the real issue reference #767"
        );
    }
}
