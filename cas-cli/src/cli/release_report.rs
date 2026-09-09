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
use chrono::{DateTime, NaiveDate, Utc};
use clap::Args;
use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use crate::bounded_process::{BoundedCommandError, Deadline, run_command};
use crate::builtins::BUILTIN_SKILLS;
use crate::cli::Cli;
use crate::config::Config;
use crate::store::find_cas_root_from;
use crate::ui::components::{Formatter, Verdict, ascii_fallback};
use crate::ui::theme::ActiveTheme;

const GH_TIMEOUT: Duration = Duration::from_secs(20);
const RENDER_TIMEOUT: Duration = Duration::from_secs(45);
const PDF_TIMEOUT: Duration = Duration::from_secs(120);
const THEME_ORDER: [&str; 7] = [
    "Factory",
    "Delivery",
    "Cloud",
    "Diagnostics",
    "MechaCassy",
    "Install",
    "Unclassified",
];

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
    theme_counts: Vec<ThemeCount>,
    warnings: Vec<String>,
    retrieved_at: String,
    github_repo: Option<String>,
    release_published_at: Option<String>,
    green_to_published_seconds: Option<i64>,
}

#[derive(Debug, Clone, Serialize)]
struct ThemeCount {
    theme: String,
    issues: usize,
    issue_numbers: Vec<u64>,
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
    tag_published_at: Option<String>,
    tag_to_published_seconds: Option<i64>,
    green_at: Option<String>,
    green_to_published_seconds: Option<i64>,
    receipt_path: Option<String>,
}

#[derive(Debug, Clone)]
struct AcquiredSources {
    project: String,
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
        let acquired = acquire_sources(&project_root, &version, &tag, &project_config, project)?;
        let source = assemble_markdown(&project_root, &version, &tag, &acquired);
        write_text(&source_path, &source)?;
        (source, acquired, true)
    };

    let renderer = RendererAssets::locate(&project_root)?;
    render_html(
        &renderer.script,
        &source_path,
        &html_path,
        &project_root,
        renderer.render_timeout,
    )?;

    let mut pdf_written = false;
    if args.pdf {
        render_pdf(&html_path, &pdf_path, &project_root)?;
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
        theme_counts: theme_counts_for_sources(&acquired),
        warnings: acquired.warnings.clone(),
        retrieved_at: acquired.retrieved_at.clone(),
        github_repo: acquired.github_repo.clone(),
        release_published_at: acquired
            .release
            .as_ref()
            .and_then(|release| release.published_at.clone())
            .or(acquired.release_evidence.tag_published_at.clone()),
        green_to_published_seconds: acquired.release_evidence.green_to_published_seconds,
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
            "{} {separator} {} issues {separator} {} warning{}",
            result.tag,
            result.issue_count,
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
    if text.is_empty() {
        return vec![String::new()];
    }
    let width = width.max(1);
    let hard_width = width.saturating_sub(1).max(1);
    let mut lines = Vec::new();
    let mut current = String::new();

    for word in text.split_whitespace() {
        let word_width = word.chars().count();
        if word_width <= width {
            if current.is_empty() {
                current.push_str(word);
            } else if current.chars().count() + 1 + word_width <= width {
                current.push(' ');
                current.push_str(word);
            } else {
                lines.push(std::mem::take(&mut current));
                current.push_str(word);
            }
            continue;
        }

        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
        }
        let mut remaining = word;
        while remaining.chars().count() > hard_width {
            let split_at = remaining
                .char_indices()
                .nth(hard_width)
                .map(|(index, _)| index)
                .unwrap_or(remaining.len());
            lines.push(remaining[..split_at].to_string());
            remaining = &remaining[split_at..];
        }
        current.push_str(remaining);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
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

    let github_repo = config.issue_repo_registry().project;
    let mut release = None;
    let mut issues = Vec::new();
    let mut assets = Vec::new();

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

        match gh_json(
            project_root,
            &[
                "pr",
                "list",
                "--repo",
                repo,
                "--state",
                "merged",
                "--search",
                tag,
                "--limit",
                "30",
                "--json",
                "number,title,body,url,closingIssuesReferences,mergedAt",
            ],
        ) {
            Ok(value) => collect_pr_issue_references(&value, &mut referenced),
            Err(error) => warnings.push(format!(
                "GitHub release PRs: {error}; remedy `gh pr list --repo {repo} --state merged`"
            )),
        }

        match gh_json(
            project_root,
            &[
                "issue",
                "list",
                "--repo",
                repo,
                "--state",
                "closed",
                "--limit",
                "1000",
                "--json",
                "number,title,url,state,closedAt,labels,body",
            ],
        ) {
            Ok(value) => {
                issues = parse_issues(&value, &referenced, repo);
                for issue in &mut issues {
                    issue.theme = classify_theme(issue);
                }
                let missing = referenced
                    .iter()
                    .filter(|number| !issues.iter().any(|issue| issue.number == **number))
                    .map(|number| format!("#{number}"))
                    .collect::<Vec<_>>();
                if !missing.is_empty() {
                    warnings.push(format!(
                        "GitHub issues: referenced closures unavailable or not closed: {}",
                        missing.join(", ")
                    ));
                }
            }
            Err(error) => warnings.push(format!(
                "GitHub issues: {error}; remedy `gh issue list --repo {repo} --state closed`"
            )),
        }
    } else {
        warnings.push(
            "GitHub sources: issues.repo is unset; remedy `cas config set issues.repo owner/name`"
                .to_string(),
        );
    }

    let release_evidence = find_release_evidence(tag, &release);
    if release_evidence.green_to_published_seconds.is_none() {
        warnings.push(
            "release receipts: green-to-published latency is unavailable under ~/.cas/artifacts/release/"
                .to_string(),
        );
    }

    let issue_count = issues.len();
    Ok(AcquiredSources {
        project,
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
            category = value.trim().to_string();
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

fn classify_theme(issue: &GithubIssue) -> String {
    let labels = issue
        .labels
        .iter()
        .map(|label| label.to_ascii_lowercase())
        .collect::<Vec<_>>();
    for (theme, keys) in [
        ("Factory", &["factory", "worker", "spawn", "supervisor"][..]),
        ("Delivery", &["delivery", "task", "queue", "message"][..]),
        ("Cloud", &["cloud", "sync", "pairing"][..]),
        (
            "Diagnostics",
            &["diagnostic", "doctor", "config", "history"][..],
        ),
        (
            "MechaCassy",
            &["mecha-cassy", "mecha_cassy", "slack", "hub"][..],
        ),
        ("Install", &["install", "release", "asset", "update"][..]),
    ] {
        if labels
            .iter()
            .any(|label| keys.iter().any(|key| label.contains(key)))
        {
            return theme.to_string();
        }
    }

    let text = format!(
        "{} {}",
        issue.title.to_ascii_lowercase(),
        issue
            .body
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
    );
    for (theme, keys) in [
        (
            "Factory",
            &["factory", "worker", "spawn", "supervisor", "watcher"][..],
        ),
        (
            "Delivery",
            &["delivery", "task", "queue", "message", "proposal"][..],
        ),
        (
            "Cloud",
            &["cloud", "sync", "pairing", "project registration"][..],
        ),
        (
            "Diagnostics",
            &[
                "diagnostic",
                "doctor",
                "config",
                "history",
                "recall",
                "index",
            ][..],
        ),
        ("MechaCassy", &["mecha-cassy", "slack", "hub", "upload"][..]),
        ("Install", &["install", "release", "asset", "update"][..]),
    ] {
        if keys.iter().any(|key| text.contains(key)) {
            return theme.to_string();
        }
    }
    "Unclassified".to_string()
}

fn theme_counts(issues: &[GithubIssue]) -> Vec<ThemeCount> {
    theme_counts_with_context(issues, None, &[])
}

fn theme_counts_for_sources(sources: &AcquiredSources) -> Vec<ThemeCount> {
    theme_counts_with_context(
        &sources.issues,
        sources.changelog.as_ref(),
        &sources.release_note_articles,
    )
}

fn theme_counts_with_context(
    issues: &[GithubIssue],
    changelog: Option<&ChangelogSection>,
    articles: &[ReleaseNoteArticle],
) -> Vec<ThemeCount> {
    let mut theme_order = draft_theme_order(articles);
    if theme_order.is_empty() {
        theme_order = THEME_ORDER
            .iter()
            .map(|theme| (*theme).to_string())
            .collect();
    }

    let mut assignments = HashMap::new();
    for article in articles {
        let theme = draft_theme_label(article);
        if !theme_order.iter().any(|known| known == &theme) {
            theme_order.push(theme.clone());
        }
        let text = format!("{} {} {}", article.title, article.was, article.now);
        for number in extract_issue_numbers(&text) {
            if issues.iter().any(|issue| issue.number == number) {
                assignments.entry(number).or_insert_with(|| theme.clone());
            }
        }
    }

    for issue in issues {
        if assignments.contains_key(&issue.number) {
            continue;
        }
        let fallback = if articles.is_empty() {
            issue.theme.clone()
        } else {
            changelog
                .and_then(|section| {
                    section
                        .entries
                        .iter()
                        .find(|entry| extract_issue_numbers(&entry.text).contains(&issue.number))
                })
                .map(|entry| entry.category.clone())
                .unwrap_or_else(|| issue.theme.clone())
        };
        if !theme_order.iter().any(|known| known == &fallback) {
            theme_order.push(fallback.clone());
        }
        assignments.insert(issue.number, fallback);
    }

    theme_order
        .into_iter()
        .map(|theme| {
            let mut issue_numbers = issues
                .iter()
                .filter(|issue| assignments.get(&issue.number) == Some(&theme))
                .map(|issue| issue.number)
                .collect::<Vec<_>>();
            issue_numbers.sort_unstable();
            ThemeCount {
                theme,
                issues: issue_numbers.len(),
                issue_numbers,
            }
        })
        .collect()
}

fn draft_theme_order(articles: &[ReleaseNoteArticle]) -> Vec<String> {
    let mut order = Vec::new();
    for article in articles {
        let theme = draft_theme_label(article);
        if !order.iter().any(|known| known == &theme) {
            order.push(theme);
        }
    }
    order
}

fn draft_theme_label(article: &ReleaseNoteArticle) -> String {
    let text = format!(
        "{} {} {} {}",
        article.title,
        article.group.as_deref().unwrap_or_default(),
        article.was,
        article.now
    )
    .to_ascii_lowercase();
    if text.contains("release") || text.contains("report") || text.contains("train") {
        "Release".to_string()
    } else if text.contains("recall")
        || text.contains("briefing")
        || text.contains("memory")
        || text.contains("budget")
        || text.contains("headroom")
    {
        "Memory".to_string()
    } else if text.contains("verification")
        || text.contains("close-gate")
        || text.contains("close gate")
        || text.contains("fair task")
        || text.contains("posture")
        || text.contains("epoch")
    {
        "Verification".to_string()
    } else if text.contains("factory")
        || text.contains("worker")
        || text.contains("spawn")
        || text.contains("start-up")
        || text.contains("startup")
        || text.contains("provider")
    {
        "Factory".to_string()
    } else {
        article
            .group
            .clone()
            .unwrap_or_else(|| "Release".to_string())
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
    let themes_for_report = theme_counts_for_sources(sources);
    let themes = themes_for_report
        .iter()
        .map(|theme| theme.theme.clone())
        .collect::<Vec<_>>();
    let theme_frontmatter = if themes.is_empty() {
        "[]".to_string()
    } else {
        format!(
            "[{}]",
            themes
                .iter()
                .map(|theme| format!("\"{}\"", theme))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let verdict = sources
        .release_note_user_punch
        .as_deref()
        .map(format_verdict)
        .unwrap_or_else(|| {
            if issue_count == 0 {
                format!("{tag} has no verified closed GitHub issues in the available release sources.")
            } else {
                format!(
                    "{tag} closes {issue_count} verified GitHub issue{} across the sourced release surfaces.",
                    if issue_count == 1 { "" } else { "s" }
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

    let map_rows = themes_for_report
        .into_iter()
        .map(|theme| {
            let ids = if theme.issue_numbers.is_empty() {
                "No listed issues".to_string()
            } else {
                theme
                    .issue_numbers
                    .iter()
                    .map(|number| format!("#{number}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            format!("| {} | {} | {} |", theme.theme, theme.issues, ids)
        })
        .collect::<Vec<_>>();
    let source_refs = source_reference_line(sources, tag);
    let theme_counts = theme_counts_for_sources(sources);
    let claim = if let Some(largest) = theme_counts
        .iter()
        .max_by_key(|theme| theme.issues)
        .filter(|theme| theme.issues > 0)
    {
        format!(
            "{} leads the closure map with {} issue{}.",
            largest.theme,
            largest.issues,
            if largest.issues == 1 { "" } else { "s" }
        )
    } else {
        "The closure map is empty until release evidence names verified issues.".to_string()
    };
    let guide = "One filled dot represents one closed issue. Dots have equal weight: this is a count of closures, not a measured impact score.";
    let glance_entries = sources
        .changelog
        .as_ref()
        .map(|section| {
            section
                .entries
                .iter()
                .filter(|entry| entry.category.eq_ignore_ascii_case("Added"))
                .count()
        })
        .unwrap_or(0);
    let fix_entries = sources
        .changelog
        .as_ref()
        .map(|section| {
            section
                .entries
                .iter()
                .filter(|entry| entry.category.eq_ignore_ascii_case("Fixed"))
                .count()
        })
        .unwrap_or(0);
    let changed_entries = sources
        .changelog
        .as_ref()
        .map(|section| {
            section
                .entries
                .iter()
                .filter(|entry| entry.category.eq_ignore_ascii_case("Changed"))
                .count()
        })
        .unwrap_or(0);
    let latency = sources
        .release_evidence
        .green_to_published_seconds
        .map(format_duration)
        .unwrap_or_else(|| "unavailable".to_string());

    let mut output = String::new();
    output.push_str("---\n");
    output.push_str(&format!("version: {version}\n"));
    output.push_str(&format!("date: {date}\n"));
    output.push_str(&format!("themes: {theme_frontmatter}\n"));
    output.push_str("---\n\n");
    output.push_str(&format!(
        "# {project} {tag}\n\n{publication_line}\n\n{verdict}\n\n"
    ));
    output.push_str("## Change map\n\n");
    output.push_str(&format!("{claim}\n\n{guide}\n\n"));
    output.push_str("| Surface | Issues closed | Issue numbers |\n| --- | ---: | --- |\n");
    output.push_str(&map_rows.join("\n"));
    output.push_str(&format!(
        "\n| Total | {issue_count} | {} issue-bearing themes |\n\n{}\n\n",
        theme_counts.iter().filter(|theme| theme.issues > 0).count(),
        source_refs
    ));
    output.push_str("## Release at a glance\n\n");
    output.push_str("| Measure | Value | Definition |\n| --- | ---: | --- |\n");
    output.push_str(&format!(
        "| Issues closed | {issue_count} | Named, verified closed issues in this report |\n"
    ));
    output.push_str(&format!(
        "| Features added | {glance_entries} | Top-level Added entries in the CHANGELOG section |\n"
    ));
    output.push_str(&format!(
        "| Changed entries | {changed_entries} | Top-level Changed entries in the CHANGELOG section |\n"
    ));
    output.push_str(&format!(
        "| Fix entries | {fix_entries} | Top-level Fixed entries in the CHANGELOG section |\n"
    ));
    output.push_str(&format!(
        "| Green to published | {latency} | gate.green.epoch to the published receipt, when available |\n\n"
    ));
    output.push_str("## What you can do now\n\n");
    output.push_str(&was_now_sections(sources, true));
    output.push_str("\n## Under the hood\n\n");
    output.push_str(&was_now_sections(sources, false));
    output.push_str("\n## Fixes ledger\n\n");
    output.push_str("| Issue | State | Closed | Theme |\n| --- | --- | --- | --- |\n");
    for issue in &sources.issues {
        output.push_str(&format!(
            "| [#{}]({}) | {} | {} | {} |\n",
            issue.number,
            issue.url,
            issue.state,
            issue.closed_at.as_deref().unwrap_or("unavailable"),
            issue.theme
        ));
    }
    output.push_str("\n## Install\n\n");
    output.push_str("```bash\n");
    output.push_str(&format!(
        "cas update  # install {tag} through the project's normal release channel\n"
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
    output.push_str(&format!("Retrieved: {}.\n\n", sources.retrieved_at));
    output.push_str(&format!("{}\n\n", source_refs));
    output.push_str(&format!(
        "GitHub issue theme assignment uses labels first, then the keyword table in the CLI. Any issue classified as `Unclassified` is shown in the map so a human can correct the Markdown before rendering.\n\n"
    ));
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
        output.push_str(&format!(
            "GitHub release: {} ({}).\n\n",
            release.url.as_deref().unwrap_or("unavailable"),
            if release.is_draft == Some(true) {
                "draft"
            } else {
                "published or unknown"
            }
        ));
    } else {
        output.push_str("GitHub release: unavailable.\n\n");
    }
    if let Some(path) = &sources.release_evidence.receipt_path {
        output.push_str(&format!("Release receipt: `{path}`.\n\n"));
    } else {
        output.push_str("Release receipt: unavailable.\n\n");
    }
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
                    article.title, article.was, article.now
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

fn source_reference_line(sources: &AcquiredSources, tag: &str) -> String {
    let mut refs = Vec::new();
    if let Some(path) = &sources.changelog_path {
        refs.push(format!("CHANGELOG `{}`", path.display()));
    }
    if let Some((path, _)) = &sources.release_notes {
        refs.push(format!("release-notes `{}`", path.display()));
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
            sources.retrieved_at
        )
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
    let value = value
        .strip_prefix("**")
        .and_then(|value| value.strip_suffix("**"))
        .or_else(|| {
            value
                .strip_prefix('*')
                .and_then(|value| value.strip_suffix('*'))
        })?
        .trim();
    if value.is_empty()
        || value.contains("Was:")
        || value.contains("Now:")
        || is_release_note_top_level(value)
    {
        return None;
    }
    Some(value.to_string())
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

fn render_html(
    script: &Path,
    source: &Path,
    output: &Path,
    project_root: &Path,
    timeout: Duration,
) -> anyhow::Result<()> {
    let mut command = Command::new("python3");
    let pdf_name = output
        .with_extension("pdf")
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "report.pdf".to_string());
    command
        .current_dir(project_root)
        .arg(script)
        .arg(source)
        .arg("--output")
        .arg(output)
        .arg("--project-root")
        .arg(project_root)
        .arg("--pdf-href")
        .arg(pdf_name);
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

fn render_pdf(html: &Path, output: &Path, project_root: &Path) -> anyhow::Result<()> {
    let node_version = Command::new("node").arg("--version").output();
    if !node_version
        .as_ref()
        .is_ok_and(|output| output.status.success())
    {
        bail!(
            "PDF renderer: Node.js is not installed; install Node.js 18+ and rerun `cas release report {} --pdf`",
            output
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("<version>")
        );
    }
    let temp = tempfile::tempdir().context("could not create PDF renderer workspace")?;
    let script = temp.path().join("render-release-report.cjs");
    fs::write(&script, PDF_SCRIPT).context("could not write PDF renderer script")?;
    let output_dir = output
        .parent()
        .ok_or_else(|| anyhow::anyhow!("PDF output has no parent directory"))?;
    let mut command = Command::new("node");
    command
        .current_dir(project_root)
        .arg(&script)
        .arg(html)
        .arg(output_dir);
    let mut output_result = run_command(&mut command, Deadline::after(PDF_TIMEOUT), PDF_TIMEOUT)
        .map_err(|error| anyhow::anyhow!(bounded_error("node", error)))?;
    if !output_result.status.success() {
        let detail = command_output_detail(&output_result);
        if playwright_module_missing(&detail) {
            // npm exposes the package's CLI through `npm exec`, but does not
            // add that package to Node's module search path for an external
            // script. Install into the disposable renderer workspace instead;
            // Playwright still uses its normal host browser cache.
            let mut npm_install = Command::new("npm");
            npm_install
                .current_dir(temp.path())
                .args(["install", "--no-save", "playwright"]);
            let install_result =
                run_command(&mut npm_install, Deadline::after(PDF_TIMEOUT), PDF_TIMEOUT)
                    .map_err(|error| anyhow::anyhow!(bounded_error("npm install", error)))?;
            if !install_result.status.success() {
                bail!(
                    "PDF renderer unavailable or failed: {}; remedy `npm install --no-save playwright` in a disposable renderer workspace",
                    command_output_detail(&install_result)
                );
            }

            let node_modules = temp.path().join("node_modules");
            let playwright_module = node_modules.join("playwright");
            let mut fallback_node = Command::new("node");
            fallback_node
                .current_dir(project_root)
                .env("NODE_PATH", &node_modules)
                .env("PLAYWRIGHT_MODULE", &playwright_module)
                .arg(&script)
                .arg(html)
                .arg(output_dir);
            output_result = run_command(
                &mut fallback_node,
                Deadline::after(PDF_TIMEOUT),
                PDF_TIMEOUT,
            )
            .map_err(|error| anyhow::anyhow!(bounded_error("node", error)))?;
        }
        if !output_result.status.success() {
            bail!(
                "PDF renderer unavailable or failed: {}; remedy `npm exec --yes --package=playwright -- node <renderer-script> <report.html> <output-dir>`",
                command_output_detail(&output_result)
            );
        }
    }
    let a4 = output_dir.join(format!(
        "{}-A4.pdf",
        html.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("report")
    ));
    if !a4.is_file() {
        bail!("PDF renderer completed without creating {}", a4.display());
    }
    fs::copy(&a4, output).with_context(|| format!("could not copy PDF to {}", output.display()))?;
    let _ = fs::remove_file(a4);
    let letter = output_dir.join(format!(
        "{}-Letter.pdf",
        html.file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("report")
    ));
    let _ = fs::remove_file(letter);
    Ok(())
}

const PDF_SCRIPT: &str = r#"const fs = require('node:fs/promises');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
(async () => {
  let playwright;
  try { playwright = require(process.env.PLAYWRIGHT_MODULE || 'playwright'); }
  catch (error) {
    console.error('Playwright is not installed:', error.message);
    process.exitCode = 2;
    return;
  }
  const [input, output] = process.argv.slice(2);
  if (!input || !output) throw new Error('expected REPORT.html OUTPUT_DIR');
  await fs.mkdir(output, { recursive: true });
  const browser = await playwright.chromium.launch({ headless: true });
  try {
    const page = await browser.newPage();
    await page.route(/^https?:/, route => route.abort());
    await page.goto(pathToFileURL(path.resolve(input)).href, { waitUntil: 'load' });
    await page.emulateMedia({ media: 'print', colorScheme: 'light' });
    await page.evaluate(async () => {
      await document.fonts.ready;
      document.querySelectorAll('details').forEach(panel => { panel.open = true; });
    });
    for (const format of ['A4', 'Letter']) {
      await page.pdf({
        path: path.join(output, path.basename(input, '.html') + '-' + format + '.pdf'),
        format, printBackground: true, preferCSSPageSize: false,
        margin: { top: '15mm', right: '15mm', bottom: '15mm', left: '15mm' },
        displayHeaderFooter: true, headerTemplate: '<span></span>',
        footerTemplate: '<div style="font:8px sans-serif;width:100%;text-align:center;color:#555"><span class="pageNumber"></span> / <span class="totalPages"></span></div>'
      });
    }
  } finally { await browser.close(); }
})().catch(error => { console.error(error.stack || error); process.exitCode = 1; });
"#;

struct RendererAssets {
    script: PathBuf,
    render_timeout: Duration,
    _temporary_root: Option<tempfile::TempDir>,
}

impl RendererAssets {
    fn locate(project_root: &Path) -> anyhow::Result<Self> {
        let relative = Path::new("skills/cas-release-report/scripts/render.py");
        let candidates = [
            project_root.join(".claude").join(relative),
            project_root.join(".codex").join(relative),
            project_root.join(".grok").join(relative),
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".claude")
                .join(relative),
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".codex")
                .join(relative),
            dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".grok")
                .join(relative),
        ];
        if let Some(script) = candidates
            .into_iter()
            .find(|path| path.is_file() && renderer_supports_frontmatter(path))
        {
            return Ok(Self {
                script,
                render_timeout: RENDER_TIMEOUT,
                _temporary_root: None,
            });
        }

        // Installed binaries and development builds both retain the skill
        // source in the builtin registry; a checkout-relative path is never
        // read at runtime (release-gate fixture-paths row).  Materialize only
        // the renderer's three read-only resources in a private temporary tree.
        let temp = tempfile::tempdir().context("could not create builtin renderer workspace")?;
        let root = temp.path().join("skills/cas-release-report");
        let references = root.join("references");
        let scripts = root.join("scripts");
        fs::create_dir_all(&references)?;
        fs::create_dir_all(&scripts)?;
        for (path, target) in [
            (
                "skills/cas-release-report/scripts/render.py",
                scripts.join("render.py"),
            ),
            (
                "skills/cas-release-report/references/template.html",
                references.join("template.html"),
            ),
            (
                "skills/cas-release-report/references/default-tokens.json",
                references.join("default-tokens.json"),
            ),
        ] {
            let builtin = BUILTIN_SKILLS
                .iter()
                .find(|file| file.path == path)
                .ok_or_else(|| anyhow::anyhow!("builtin renderer resource missing: {path}"))?;
            fs::write(&target, builtin.content)?;
        }
        Ok(Self {
            script: scripts.join("render.py"),
            render_timeout: RENDER_TIMEOUT,
            _temporary_root: Some(temp),
        })
    }
}

fn renderer_supports_frontmatter(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|content| content.contains("front matter needs a closing"))
        .unwrap_or(false)
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

fn find_release_evidence(tag: &str, release: &Option<ReleaseMetadata>) -> ReleaseEvidence {
    let Some(home) = dirs::home_dir() else {
        return ReleaseEvidence::default();
    };
    let root = home.join(".cas/artifacts/release");
    let wanted = tag.to_ascii_lowercase();
    let bare = wanted.trim_start_matches('v');
    let mut files = Vec::new();
    collect_files(&root, &mut files);
    files.sort();
    let mut evidence = ReleaseEvidence::default();
    let mut green_epoch = None;
    let mut published_at = release
        .as_ref()
        .and_then(|release| release.published_at.clone());
    for path in files {
        let path_text = path.to_string_lossy().to_ascii_lowercase();
        if !path_text.contains(&wanted) && !path_text.contains(bare) {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        if path.file_name().and_then(|name| name.to_str()) == Some("gate.green.epoch") {
            green_epoch = content.trim().parse::<i64>().ok();
        }
        let values = parse_key_values(&content);
        if let Some(value) = values.get("PUBLISHED_AT") {
            published_at = Some(value.clone());
        }
        if let Some(value) = values.get("PUBLISH_LATENCY_SECONDS") {
            evidence.tag_to_published_seconds = value.parse().ok();
        }
        if values.contains_key("PUBLISH_LATENCY_SECONDS") || path_text.contains("published.receipt")
        {
            evidence.receipt_path = Some(path.display().to_string());
        }
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
    evidence
}

fn parse_key_values(content: &str) -> HashMap<String, String> {
    content
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .collect()
}

fn extract_issue_numbers(text: &str) -> Vec<u64> {
    let regex = Regex::new(r"#([0-9]+)\b").expect("issue reference regex");
    regex
        .captures_iter(text)
        .filter_map(|capture| capture.get(1)?.as_str().parse().ok())
        .collect()
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

fn format_publication_line(value: &str) -> String {
    DateTime::parse_from_rfc3339(value)
        .map(|timestamp| {
            let timestamp = timestamp.with_timezone(&Utc);
            format!(
                "Published {} · {} UTC",
                timestamp.format("%-d %B %Y"),
                timestamp.format("%H:%M")
            )
        })
        .unwrap_or_else(|_| format!("Published {value}"))
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
        heading.truncate(97);
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

fn playwright_module_missing(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("playwright is not installed")
        || detail.contains("cannot find module 'playwright'")
        || detail.contains("cannot find module \"playwright\"")
}

#[cfg(test)]
mod tests {
    use super::*;

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

        let sources = AcquiredSources {
            project: "fixture".to_string(),
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
        let user = was_now_sections(&sources, true);
        let dev = was_now_sections(&sources, false);
        assert!(user.contains("Readable report"));
        assert!(!user.contains("report sources were gathered manually"));
        assert!(!dev.contains("report sources were gathered manually"));
        assert!(dev.contains("Assembler detail"));
        assert!(!dev.contains("Readable report"));
    }

    #[test]
    fn release_note_groups_drive_verdict_themes_and_issue_assignment() {
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

        let changelog = parse_changelog_section(
            "# Changelog\n\n## [2.4.0] - 2026-09-01\n\n### Changed\n- Close gate repair (#767)\n",
            "2.4.0",
        )
        .expect("section");
        let issue = GithubIssue {
            number: 767,
            title: "Close gate repair".to_string(),
            url: "https://example.test/issues/767".to_string(),
            state: "CLOSED".to_string(),
            closed_at: Some("2026-09-09T14:47:28Z".to_string()),
            labels: Vec::new(),
            body: None,
            theme: "Unclassified".to_string(),
        };
        let counts = theme_counts_with_context(&[issue], Some(&changelog), &parsed.articles);
        assert_eq!(
            counts
                .iter()
                .map(|theme| theme.theme.as_str())
                .collect::<Vec<_>>(),
            vec!["Release", "Memory", "Verification", "Factory"]
        );
        assert_eq!(counts[2].issue_numbers, vec![767]);
        assert_eq!(
            format_publication_line("2026-09-09T16:51:40Z"),
            "Published 9 September 2026 · 16:51 UTC"
        );
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
    fn playwright_module_failure_is_selected_for_npm_fallback() {
        assert!(playwright_module_missing(
            "Playwright is not installed: Cannot find module 'playwright'"
        ));
        assert!(playwright_module_missing(
            "Error: Cannot find module \"playwright\""
        ));
        assert!(!playwright_module_missing(
            "browserType.launch: Executable doesn't exist"
        ));
    }

    #[test]
    fn issue_fixture_parses_labels_and_assets_preserve_digests() {
        let issues = serde_json::json!([
            {"number": 12, "title": "Build worker", "url": "https://github.com/o/r/issues/12", "state": "CLOSED", "closedAt": "2026-09-01T01:02:03Z", "labels": [{"name": "factory"}], "body": "worker"},
            {"number": 13, "title": "Mysterious change", "url": "https://github.com/o/r/issues/13", "state": "CLOSED", "closedAt": null, "labels": []}
        ]);
        let references = [12_u64, 13].into_iter().collect();
        let mut parsed = parse_issues(&issues, &references, "o/r");
        parsed[0].theme = classify_theme(&parsed[0]);
        parsed[1].theme = classify_theme(&parsed[1]);
        assert_eq!(parsed[0].theme, "Factory");
        assert_eq!(parsed[1].theme, "Unclassified");

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

    #[test]
    fn map_counts_keep_zero_install_and_unclassified_rows() {
        let issue = GithubIssue {
            number: 1,
            title: "unknown".to_string(),
            url: "https://example/1".to_string(),
            state: "CLOSED".to_string(),
            closed_at: None,
            labels: Vec::new(),
            body: None,
            theme: "Unclassified".to_string(),
        };
        let counts = theme_counts(&[issue]);
        assert_eq!(counts.len(), THEME_ORDER.len());
        assert_eq!(counts[5].theme, "Install");
        assert_eq!(counts[5].issues, 0);
        assert_eq!(counts[6].issue_numbers, vec![1]);
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
            theme_counts: Vec::new(),
            warnings: vec![
                "source preserved: /home/pippenz/.cas/artifacts/cas-7dfa/reports/v3.19.0.md is authoritative for v3.19.0".to_string(),
            ],
            retrieved_at: "2026-09-09T15:00:00Z".to_string(),
            github_repo: None,
            release_published_at: None,
            green_to_published_seconds: None,
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
            theme_counts: Vec::new(),
            warnings: vec!["source preserved: docs/release-reports/v3.19.0.md".to_string()],
            retrieved_at: "2026-09-09T15:00:00Z".to_string(),
            github_repo: None,
            release_published_at: None,
            green_to_published_seconds: None,
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
}
