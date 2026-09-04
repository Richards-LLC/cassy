//! Self-update command for Cassy CLI
//!
//! Downloads and installs the latest version from GitHub releases,
//! and runs schema migrations for the local database.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Args;

use crate::builtins::{
    SyncResult, ensure_builtin_gitignore, mark_missing_owned_references_for_replacement,
    prune_stale_user_skills_for_harness, sync_all_builtins_for_harness,
    sync_all_builtins_for_project,
};
use crate::cli::Cli;
use crate::cli::cloud::{
    CloudSyncArgs, SyncSummary, execute_sync_with_summaries, render_sync_summary,
};
use crate::cli::factory_tooling;
use crate::cli::hook::{
    configure_claude_hooks, configure_mcp_server, provision_codex_project,
    provision_codex_user_config,
};
use crate::cli::init::{generate_cas_skill, update_claude_md};
use crate::cli::update::preview::{build_update_transaction, show_enhanced_dry_run};
use crate::cloud::{CloudConfig, FetchTeamsOutcome, fetch_and_cache_teams, maybe_adopt_team_scope};
use crate::hybrid_search::{LegacyRepairLimits, LegacyRepairOutcome, repair_legacy_index_bounded};
use crate::migration::{check_migrations, run_migrations};
use crate::store::{open_rule_store, open_skill_store, open_store};
use crate::sync::{SkillSyncer, Syncer};
use crate::ui::components::table::Column as TableColumn;
use crate::ui::components::{Border, Formatter, OutputMode, Renderable, Table, Width};
use crate::ui::theme::ActiveTheme;

mod preview;

fn report_modified_builtin_references(
    result: &SyncResult,
    location: &str,
    theme: &ActiveTheme,
) -> std::io::Result<()> {
    if !result.has_modified_references() {
        return Ok(());
    }

    let mut out = io::stdout();
    let mut fmt = Formatter::stdout(&mut out, theme.clone());
    fmt.write_raw("  ")?;
    fmt.warning(&format!(
        "{} locally modified builtin reference file(s) in {location} were preserved \
         (content matches no version Cassy has shipped):",
        result.modified_reference_files.len()
    ))?;
    for file in &result.modified_reference_files {
        fmt.write_raw(&format!("    ! {file}"))?;
        fmt.newline()?;
    }
    fmt.write_raw("    ")?;
    fmt.write_raw(
        "Review each file; to accept the Cassy version, delete it and rerun `cas update --sync`.",
    )?;
    fmt.newline()
}

/// Report one harness's builtin sync result, attributed to the harness
/// directory it actually wrote to (cas-27bf).
///
/// This MUST be called immediately after that harness's
/// `sync_all_builtins_for_harness` call, inside the same `Syncing <dir> files`
/// section. Previously the Claude result was rendered in a trailing block that
/// ran after the `.codex` / `.grok` subheadings had already been printed, so a
/// Claude-only write was displayed as though Codex had performed it — a
/// claimed-green/did-nothing shape — while the Codex and Grok counts were never
/// printed in human mode at all. Every count and `+ path` line emitted here is
/// derived from `SyncResult`, whose counters only advance after a successful
/// `std::fs::write` (see `builtins::sync_all_builtins_inner`); IO failures
/// propagate as errors and abort the run non-zero.
fn report_builtin_sync(
    result: &SyncResult,
    location: &str,
    theme: &ActiveTheme,
) -> std::io::Result<()> {
    {
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme.clone());

        if result.total_updated() > 0 {
            fmt.write_raw("  ")?;
            fmt.success(&format!(
                "{location}: updated {} built-in files ({} agents, {} skills)",
                result.total_updated(),
                result.agents_updated,
                result.skills_updated
            ))?;
            for file in &result.updated_files {
                fmt.write_raw(&format!("    + {location}/{file}"))?;
                fmt.newline()?;
            }
        } else {
            fmt.write_raw("  ")?;
            fmt.success(&format!("{location}: built-ins up to date"))?;
        }

        // cas-4900: surface silent skips so stale destinations stop
        // accumulating invisibly. Each entry here is a file whose
        // on-disk content differs from the source but lacks
        // `managed_by: cas` in either frontmatter, so the gate refused
        // to overwrite. Pre-9362ee0 this whole class of files was
        // silently skipped with no signal whatsoever; now the user
        // sees the list and can decide.
        if result.has_silent_skips() {
            fmt.write_raw("  ")?;
            fmt.warning(&format!(
                "{} file(s) at {location} differ from source but were NOT updated \
                 because neither side carries `managed_by: cas` frontmatter (cas-4900):",
                result.skipped_files.len()
            ))?;
            for file in &result.skipped_files {
                fmt.write_raw(&format!("    ! {location}/{file}"))?;
                fmt.newline()?;
            }
            fmt.write_raw("    ")?;
            fmt.write_raw("(add `managed_by: cas` to the source frontmatter to enable updates)")?;
            fmt.newline()?;
        }
    }

    report_modified_builtin_references(result, location, theme)
}

/// GitHub repository owner
const REPO_OWNER: &str = "pippenz";

/// GitHub repository name
const REPO_NAME: &str = "cas";

/// Binary name in release assets
const BIN_NAME: &str = "cas";

/// An automatic update must not hang indefinitely behind a pre-update daemon.
/// The repair primitive itself also bounds lock acquisition, but this outer
/// deadline closes the reader-acquisition race described in legacy_index.rs.
const UPDATE_REPAIR_BUDGET: Duration = Duration::from_secs(20);

#[derive(Args)]
pub struct UpdateArgs {
    /// Only check for updates without installing
    #[arg(long)]
    pub check: bool,

    /// Update to a specific version (e.g., "0.2.1")
    #[arg(long)]
    pub version: Option<String>,

    /// Skip confirmation prompt
    #[arg(short = 'y', long)]
    pub yes: bool,

    /// Only run schema migrations (skip binary update)
    #[arg(long)]
    pub schema_only: bool,

    /// Only sync .claude/.codex files (agents, skills, rules, settings)
    #[arg(long)]
    pub sync: bool,

    /// Distribute embedded built-in skills/agents/commands to ~/.claude
    /// (and ~/.codex if present). Does not touch project-scoped config
    /// (settings.json, CLAUDE.md, hooks, db-backed rules/skills).
    #[arg(long)]
    pub user: bool,

    /// Show what migrations would be applied without running them
    #[arg(long)]
    pub dry_run: bool,

    /// Keep backup files after successful update
    #[arg(long)]
    pub keep_backup: bool,

    /// Refresh every discovered local Cassy project after updating.
    ///
    /// Performs schema migration, generated-file/builtin sync, cloud team
    /// membership refresh, legacy search-index repair, and cloud sync for
    /// every cloud-linked project.
    #[arg(long)]
    pub all_projects: bool,

    /// Register an existing project in the host registry so every later
    /// `cas update` refreshes it without relying on the filesystem scan.
    #[arg(long, value_name = "PATH")]
    pub register: Option<PathBuf>,

    /// Run the post-swap hook from a freshly installed binary.
    #[arg(long = "post-swap", hide = true)]
    pub post_swap: bool,

    /// Version replaced by the post-swap invocation.
    #[arg(long = "from", hide = true, requires = "post_swap")]
    pub from: Option<String>,
}

pub fn execute(args: &UpdateArgs, cli: &Cli, cas_root: Option<&Path>) -> anyhow::Result<()> {
    // Note: update command accepts Option<&Path> because it can run without an initialized Cassy
    // (e.g., binary update only, or checking for updates before init)
    let current_version = env!("CARGO_PKG_VERSION");

    // A post-swap invocation is dispatched by the newly installed binary. It
    // must terminate before any path that can download or install another
    // binary, otherwise every update would recursively launch updates.
    if args.post_swap {
        return execute_post_swap(args, cli, current_version);
    }

    if let Some(path) = &args.register {
        return register_project(path, cli);
    }

    // This is also the no-download entry point for a host which already has
    // the desired binary. It deliberately does the same complete sweep that a
    // successful ordinary `cas update` performs below.
    if args.all_projects {
        let mut steps = UpdateStepTracker::new(1, !cli.json);
        let report = steps.run("Refreshing all local Cassy projects", || {
            refresh_all_projects(args, cli, cas_root)
        })?;
        if !cli.json {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
            print_update_banner_with_formatter(&mut fmt, &report)?;
        }
        return Ok(());
    }

    // Handle user-level builtin distribution (~/.claude, ~/.codex)
    if args.user {
        let mut steps = UpdateStepTracker::new(1, !cli.json);
        return steps.run("Distributing built-ins to user-level", || {
            sync_user_builtins(cli)
        });
    }

    // Handle sync-only mode (just sync .claude/.codex files)
    if args.sync {
        let mut steps = UpdateStepTracker::new(1, !cli.json);
        return steps.run("Syncing .claude/.codex files", || {
            sync_claude_files(cli, cas_root)
        });
    }

    // Handle schema-only mode
    if args.schema_only || args.dry_run {
        let mut steps = UpdateStepTracker::new(1, !cli.json);
        return steps.run("Applying schema updates", || {
            run_schema_migrations(args, cli, cas_root)
        });
    }

    // Handle check mode (includes schema status)
    if args.check {
        return check_for_updates(current_version, cli, cas_root);
    }

    // Full update: binary + every local project's migration/sync/cloud state.
    let mut steps = UpdateStepTracker::new(2, !cli.json);
    steps.run("Updating Cassy binary", || {
        let installed_version = perform_update(args, current_version, cli)?;
        super::hub::restart_stale_hub(&installed_version, cli)?;
        Ok(installed_version)
    })?;
    if !cli.json {
        let mut out = io::stdout();
        let theme = ActiveTheme::default();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.newline()?;
    }

    let report = steps.run("Refreshing all local Cassy projects", || {
        refresh_all_projects(args, cli, cas_root)
    })?;

    if !cli.json {
        let mut out = io::stdout();
        let theme = ActiveTheme::default();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.newline()?;
        print_update_banner_with_formatter(&mut fmt, &report)?;
    }

    Ok(())
}

/// Add an existing project to the host registry.
///
/// The escape hatch for a project the scan cannot reach — one outside `$HOME`
/// and outside `CAS_PROJECT_ROOTS`, or nested deeper than [`MAX_SCAN_DEPTH`].
/// Registration is what stops the project from depending on the scan at all.
fn register_project(path: &Path, cli: &Cli) -> anyhow::Result<()> {
    let project = canonical_path(path);
    if !project.join(".cas").is_dir() {
        anyhow::bail!(
            "{} is not a Cassy project (no .cas directory) — run `cas init` there first",
            project.display()
        );
    }
    crate::store::known_repos::ensure_host_schema()
        .context("could not open the host known-repo registry")?;
    crate::store::known_repos::register_repo_strict(&project)
        .with_context(|| format!("could not register {}", project.display()))?;

    if cli.json {
        println!(
            "{}",
            serde_json::json!({ "registered": project, "store": project.join(".cas") })
        );
    } else {
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
        fmt.success(&format!(
            "Registered {} — future `cas update` runs refresh it",
            project.display()
        ))?;
    }
    Ok(())
}

/// Outcome printed in the final all-projects receipt. Phase failures are kept
/// as data so one bad checkout never prevents the remaining projects from
/// becoming current.
#[derive(Debug, Clone)]
enum ProjectPhase {
    Ok(String),
    Skipped(String),
    Planned(String),
    Warning(String),
    Failed(String),
}

impl ProjectPhase {
    fn summary(&self) -> String {
        match self {
            Self::Ok(detail) => format!("ok: {detail}"),
            Self::Skipped(detail) => format!("skipped: {detail}"),
            Self::Planned(detail) => format!("dry-run: {detail}"),
            Self::Warning(detail) => format!("warning: {detail}"),
            Self::Failed(detail) => format!("FAILED: {detail}"),
        }
    }

    fn failed(&self) -> bool {
        matches!(self, Self::Failed(_))
    }

    fn is_ok(&self) -> bool {
        matches!(self, Self::Ok(_))
    }

    fn glyph(&self) -> &'static str {
        match self {
            Self::Ok(_) => "✓",
            Self::Warning(_) => "⚠",
            Self::Failed(_) => "✗",
            Self::Skipped(_) | Self::Planned(_) => "–",
        }
    }

    fn detail(&self) -> &str {
        match self {
            Self::Ok(detail)
            | Self::Skipped(detail)
            | Self::Planned(detail)
            | Self::Warning(detail)
            | Self::Failed(detail) => detail,
        }
    }

    fn severity(&self) -> u8 {
        match self {
            Self::Failed(_) => 3,
            Self::Warning(_) => 2,
            Self::Skipped(_) | Self::Planned(_) => 1,
            Self::Ok(_) => 0,
        }
    }

    fn status_label(&self) -> &'static str {
        match self {
            Self::Ok(_) => "[OK]",
            Self::Skipped(_) => "[SKIP]",
            Self::Planned(_) => "[DRY]",
            Self::Warning(_) => "[WARN]",
            Self::Failed(_) => "[ERROR]",
        }
    }
}

struct ProjectRefreshReceipt {
    project: PathBuf,
    /// Found by the filesystem scan rather than the host registry. Refreshed
    /// exactly like any other project, then registered so the next run finds
    /// it without scanning.
    unregistered: bool,
    migration: ProjectPhase,
    search_index: ProjectPhase,
    skills: ProjectPhase,
    membership: ProjectPhase,
    cloud: ProjectPhase,
    details: String,
    phase_details: Vec<(bool, String)>,
}

impl ProjectRefreshReceipt {
    fn failed(&self) -> bool {
        [&self.migration, &self.skills, &self.membership, &self.cloud]
            .into_iter()
            .any(ProjectPhase::failed)
    }
}

struct RefreshReport {
    project_count: usize,
    failed_count: usize,
    /// Directories carrying a `.cas/` but no store. Counted in the banner so a
    /// refresh can never claim a clean sweep while leaving stores untouched.
    skipped_unregistered: usize,
    elapsed: Duration,
}

#[derive(Default)]
struct RepeatedWarning {
    projects: BTreeSet<String>,
    paths: BTreeSet<String>,
}

#[derive(Default)]
struct RepeatedWarningCollector {
    warnings: BTreeMap<String, RepeatedWarning>,
}

impl RepeatedWarningCollector {
    fn record(&mut self, warning: &str, project: &str) {
        self.warnings
            .entry(warning.to_owned())
            .or_default()
            .projects
            .insert(project.to_owned());
    }

    fn record_builtin_paths<I, S>(&mut self, project: &str, paths: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let warning = self
            .warnings
            .entry("Cassy-managed builtin paths already tracked".to_owned())
            .or_default();
        warning.projects.insert(project.to_owned());
        warning.paths.extend(paths.into_iter().map(Into::into));
    }

    fn collect_output(&mut self, project: &str, output: &str) {
        let mut builtin_warning = false;
        for line in output.lines() {
            let raw_line = line;
            let line = raw_line.trim();
            let normalized = line
                .strip_prefix("[WARN] ")
                .or_else(|| line.strip_prefix("[ERROR] "))
                .or_else(|| line.strip_prefix("! "))
                .unwrap_or(line)
                .trim();
            if normalized.contains("Cassy-managed builtin path(s) are already tracked") {
                builtin_warning = true;
                self.record_builtin_paths(project, std::iter::empty::<String>());
            } else if normalized.starts_with("Push incomplete") {
                self.record("Push incomplete; queued rows remain", project);
            } else if let Some(error) = normalized.strip_prefix("remaining error: ") {
                self.record(&format!("remaining error: {error}"), project);
            } else if builtin_warning && let Some(path) = raw_line.trim_start().strip_prefix('!') {
                self.record_builtin_paths(project, [path.trim().to_owned()]);
            } else if builtin_warning
                && (line.starts_with("To make") || line.starts_with("Review each file"))
            {
                continue;
            } else if builtin_warning && !line.is_empty() {
                builtin_warning = false;
            }
        }
    }

    fn render(&self, verbose: bool) -> String {
        if verbose {
            return String::new();
        }
        let mut output = String::new();
        for (message, warning) in &self.warnings {
            let count = warning.projects.len();
            output.push_str(&format!(
                "[WARN] {message} ({count} {})\n",
                if count == 1 { "project" } else { "projects" }
            ));
            let paths = warning.paths.iter();
            let limit = if verbose { usize::MAX } else { 5 };
            for path in paths.take(limit) {
                output.push_str(&format!("  ! {path}\n"));
            }
            if warning.paths.len() > limit {
                output.push_str(&format!(
                    "  … and {} more path(s)\n",
                    warning.paths.len() - limit
                ));
            }
        }
        output
    }
}

fn project_display_name(path: &Path, verbose: bool) -> String {
    if verbose {
        return path.display().to_string();
    }
    let components = path
        .components()
        .filter_map(|component| component.as_os_str().to_str())
        .collect::<Vec<_>>();
    components
        .get(components.len().saturating_sub(2)..)
        .unwrap_or(&components)
        .join("/")
}

fn project_note(receipt: &ProjectRefreshReceipt) -> String {
    let phases = [
        ("migration", &receipt.migration),
        ("index", &receipt.search_index),
        ("skills", &receipt.skills),
        ("member", &receipt.membership),
        ("cloud", &receipt.cloud),
    ];
    let non_ok =
        phases
            .iter()
            .filter(|(_, phase)| !phase.is_ok())
            .fold(None, |most_severe, candidate| match most_severe {
                None => Some(candidate),
                Some(current) if candidate.1.severity() > current.1.severity() => Some(candidate),
                Some(current) => Some(current),
            });
    if let Some((label, phase)) = non_ok {
        let detail = phase.detail().trim();
        let reason = if detail.is_empty() {
            format!("{label} {}", phase.status_label().trim_matches(['[', ']']))
        } else {
            detail.to_owned()
        };
        return shorten_project_note(&reason);
    }

    let detail = phases
        .iter()
        .find(|(_, phase)| !phase.detail().is_empty())
        .map(|(_, phase)| phase.detail())
        .unwrap_or_default();
    if detail.contains("not cloud-linked") {
        return "not cloud-linked".to_owned();
    }
    if let Some(version) = version_token(detail).or_else(|| version_token(&receipt.details)) {
        return version;
    }
    shorten_project_note(detail)
}

fn shorten_project_note(detail: &str) -> String {
    if detail.chars().count() > 28 {
        let short = detail.chars().take(28).collect::<String>();
        format!("{short}…")
    } else {
        detail.to_owned()
    }
}

fn version_token(text: &str) -> Option<String> {
    text.split(|character: char| !character.is_ascii_alphanumeric())
        .find(|word| {
            word.starts_with('v')
                && word.len() > 1
                && word[1..]
                    .chars()
                    .all(|character| character.is_ascii_digit())
        })
        .map(str::to_owned)
}

fn project_table(receipts: &[ProjectRefreshReceipt], verbose: bool) -> Table {
    let rows = receipts
        .iter()
        .map(|receipt| {
            vec![
                project_display_name(&receipt.project, verbose),
                receipt.migration.glyph().to_owned(),
                receipt.search_index.glyph().to_owned(),
                receipt.skills.glyph().to_owned(),
                receipt.membership.glyph().to_owned(),
                receipt.cloud.glyph().to_owned(),
                project_note(receipt),
            ]
        })
        .collect::<Vec<_>>();
    Table::new()
        .columns_detailed(vec![
            TableColumn::new("project"),
            TableColumn::new("migr").width(Width::Min(4)),
            TableColumn::new("index").width(Width::Min(5)),
            TableColumn::new("skills").width(Width::Min(6)),
            TableColumn::new("member").width(Width::Min(6)),
            TableColumn::new("cloud").width(Width::Min(5)),
            TableColumn::new("note"),
        ])
        .rows(rows)
        .border(Border::None)
        .indent(2)
}

fn render_project_table_at_width(
    receipts: &[ProjectRefreshReceipt],
    verbose: bool,
    mode: OutputMode,
    width: u16,
) -> String {
    let table = project_table(receipts, verbose);
    let mut bytes = Vec::new();
    {
        let mut fmt = Formatter::new(&mut bytes, mode, ActiveTheme::default(), width);
        table.render(&mut fmt).expect("project table cannot fail");
    }
    String::from_utf8(bytes).expect("project table is UTF-8")
}

fn render_project_table_plain(receipts: &[ProjectRefreshReceipt], verbose: bool) -> String {
    render_project_table_at_width(receipts, verbose, OutputMode::Plain, 80)
}

fn strip_repeated_warning_lines(
    output: &str,
    warnings: &mut RepeatedWarningCollector,
    project: &str,
    verbose: bool,
    preserve_phase_summaries: bool,
) -> String {
    warnings.collect_output(project, output);
    if verbose {
        return output.to_owned();
    }
    let mut kept = String::new();
    let mut builtin_warning = false;
    for line in output.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("[Cassy sync] Conflict resolved:")
            || trimmed.starts_with("[Cassy sync] Starting team pull:")
            || trimmed.starts_with("healed ")
        {
            continue;
        }
        let normalized = line
            .trim_start()
            .strip_prefix("[WARN] ")
            .unwrap_or(line.trim());
        if normalized.contains("Cassy-managed builtin path(s) are already tracked")
            || normalized.starts_with("Push incomplete")
            || normalized.starts_with("remaining error: ")
        {
            builtin_warning = normalized.contains("Cassy-managed builtin");
            if preserve_phase_summaries
                && (normalized.starts_with("Push incomplete")
                    || normalized.starts_with("remaining error: "))
            {
                kept.push_str(line);
                kept.push('\n');
            }
            continue;
        }
        if builtin_warning && (line.trim_start().starts_with('!') || line.trim().is_empty()) {
            continue;
        }
        if builtin_warning && line.trim_start().starts_with("To make") {
            builtin_warning = false;
            continue;
        }
        kept.push_str(line);
        kept.push('\n');
    }
    kept
}

fn render_project_phase_details(
    receipt: &ProjectRefreshReceipt,
    verbose: bool,
    warnings: &mut RepeatedWarningCollector,
    project: &str,
) -> String {
    let phases = [
        ("migration", &receipt.migration),
        ("index", &receipt.search_index),
        ("skills", &receipt.skills),
        ("member", &receipt.membership),
        ("cloud", &receipt.cloud),
    ];
    let mut selected = String::new();
    for (index, (label, phase)) in phases.into_iter().enumerate() {
        let (captured_ok, output) = receipt
            .phase_details
            .get(index)
            .map(|(is_ok, output)| (*is_ok, output.as_str()))
            .unwrap_or((phase.is_ok(), ""));
        if verbose || !captured_ok {
            if output.trim().is_empty() && !phase.is_ok() {
                selected.push_str(&format!(
                    "{} {label}: {}\n",
                    phase.status_label(),
                    phase.detail()
                ));
            } else {
                selected.push_str(output);
            }
        }
    }
    strip_repeated_warning_lines(&selected, warnings, project, verbose, true)
}

fn update_banner_text(report: &RefreshReport) -> String {
    let skipped = if report.skipped_unregistered > 0 {
        format!(
            " · {} unregistered store(s) not refreshed",
            report.skipped_unregistered
        )
    } else {
        String::new()
    };
    format!(
        "Cassy {} · {} projects refreshed · {} failed{skipped} · {}",
        env!("CARGO_PKG_VERSION"),
        report.project_count,
        report.failed_count,
        format_elapsed(report.elapsed)
    )
}

fn print_update_banner_with_formatter(
    fmt: &mut Formatter<'_>,
    report: &RefreshReport,
) -> io::Result<()> {
    fmt.success(&update_banner_text(report))
}

fn capture_phase<T>(enabled: bool, operation: impl FnOnce() -> T) -> (T, String) {
    if !enabled {
        return (operation(), String::new());
    }

    #[cfg(unix)]
    {
        if let Ok(mut capture) = OutputCapture::new() {
            let value = operation();
            let output = capture.finish();
            return (value, output);
        }
    }

    (operation(), String::new())
}

#[cfg(unix)]
struct OutputCapture {
    stdout_backup: RawFd,
    stderr_backup: RawFd,
    file: std::fs::File,
}

#[cfg(unix)]
impl OutputCapture {
    fn new() -> io::Result<Self> {
        let file = tempfile::tempfile()?;
        let stdout_backup = unsafe { libc::dup(libc::STDOUT_FILENO) };
        if stdout_backup < 0 {
            return Err(io::Error::last_os_error());
        }
        let stderr_backup = unsafe { libc::dup(libc::STDERR_FILENO) };
        if stderr_backup < 0 {
            unsafe { libc::close(stdout_backup) };
            return Err(io::Error::last_os_error());
        }
        let fd = file.as_raw_fd();
        if unsafe { libc::dup2(fd, libc::STDOUT_FILENO) } < 0
            || unsafe { libc::dup2(fd, libc::STDERR_FILENO) } < 0
        {
            unsafe {
                libc::dup2(stdout_backup, libc::STDOUT_FILENO);
                libc::dup2(stderr_backup, libc::STDERR_FILENO);
                libc::close(stdout_backup);
                libc::close(stderr_backup);
            }
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            stdout_backup,
            stderr_backup,
            file,
        })
    }

    fn finish(&mut self) -> String {
        let _ = io::stdout().flush();
        let _ = io::stderr().flush();
        let mut output = String::new();
        let _ = self.file.seek(SeekFrom::Start(0));
        let _ = self.file.read_to_string(&mut output);
        output
    }
}

#[cfg(unix)]
impl Drop for OutputCapture {
    fn drop(&mut self) {
        unsafe {
            libc::dup2(self.stdout_backup, libc::STDOUT_FILENO);
            libc::dup2(self.stderr_backup, libc::STDERR_FILENO);
            libc::close(self.stdout_backup);
            libc::close(self.stderr_backup);
        }
    }
}

/// Implement the post-update host sweep natively. The old contrib helper
/// spawned `cas update --schema-only` and `cas update --sync` once per path;
/// doing it in-process avoids source-checkout-only behavior and lets cloud
/// phases operate on explicit project roots rather than the updater's cwd.
fn refresh_all_projects(
    args: &UpdateArgs,
    cli: &Cli,
    current_cas_root: Option<&Path>,
) -> anyhow::Result<RefreshReport> {
    let started_at = Instant::now();
    let discovery = discover_local_projects(current_cas_root);
    let mut receipts = Vec::with_capacity(discovery.projects.len());

    for project in &discovery.projects {
        let project = project.clone();
        let cas_root = project.join(".cas");
        let mut details = String::new();

        // Run each phase independently. A malformed database must be visible
        // in the receipt, but must not leave another project stale.
        let (migration, output) = capture_phase(!cli.json, || {
            run_project_phase("migration", args.dry_run, || {
                run_schema_migrations(args, cli, Some(&cas_root))
            })
        });
        details.push_str(&output);
        let mut phase_details = vec![(migration.is_ok(), output)];
        let (search_index, output) = capture_phase(!cli.json, || {
            repair_project_search_index(&cas_root, args.dry_run, cli)
        });
        details.push_str(&output);
        phase_details.push((search_index.is_ok(), output));
        let (skills, output) = capture_phase(!cli.json, || {
            run_project_phase("skills", args.dry_run, || {
                sync_claude_files(cli, Some(&cas_root))
            })
        });
        details.push_str(&output);
        phase_details.push((skills.is_ok(), output));
        let (membership, output) = capture_phase(!cli.json, || {
            refresh_project_membership(&cas_root, args.dry_run)
        });
        details.push_str(&output);
        phase_details.push((membership.is_ok(), output));
        let ((cloud, _summaries), output) = capture_phase(!cli.json, || {
            sync_project_cloud(&cas_root, args.dry_run, cli)
        });
        details.push_str(&output);
        phase_details.push((cloud.is_ok(), output));

        // Registration is what makes discovery converge: a project the scan had
        // to find this time is in the registry for every later run, and for
        // every other Cassy entry point that reads it.
        let unregistered = discovery.unregistered.contains(&project);
        if unregistered && !args.dry_run && !migration.failed() {
            crate::store::known_repos::register_repo(&project);
        }

        receipts.push(ProjectRefreshReceipt {
            project,
            unregistered,
            migration,
            search_index,
            skills,
            membership,
            cloud,
            details,
            phase_details,
        });
    }

    let (user_level, user_details) =
        capture_phase(!cli.json, || refresh_user_level_store(args, cli));
    print_project_refresh_summary(
        &receipts,
        &user_level,
        &user_details,
        &discovery.skipped_unregistered,
        cli,
    );

    let failed_count = receipts.iter().filter(|receipt| receipt.failed()).count()
        + usize::from(user_level.failed());
    if failed_count > 0 {
        anyhow::bail!(
            "one or more projects were not fully refreshed; see the per-project phase summary above"
        );
    }
    Ok(RefreshReport {
        project_count: receipts.len(),
        failed_count,
        skipped_unregistered: discovery.skipped_unregistered.len(),
        elapsed: started_at.elapsed(),
    })
}

/// Refresh the user-level store (`~/.cas`) as its own phase.
///
/// Before cas-9d5c the all-projects sweep distributed user-level builtins but
/// never ran migrations on `~/.cas`, so the host store drifted behind every
/// project on the machine while the receipt reported a clean run. It is
/// deliberately not a project: it is never counted in the project totals and
/// never appears in the project table.
fn refresh_user_level_store(args: &UpdateArgs, cli: &Cli) -> ProjectPhase {
    let Some(cas_root) = user_level_store_root() else {
        return ProjectPhase::Skipped("no home directory".to_string());
    };
    if !cas_root.join("cas.db").is_file() {
        return match sync_user_builtins(cli) {
            Ok(()) => ProjectPhase::Ok(format!(
                "builtins only — no store at {}",
                cas_root.display()
            )),
            Err(error) => ProjectPhase::Failed(error.to_string()),
        };
    }

    if args.dry_run {
        return ProjectPhase::Planned(format!("migrate {} and sync builtins", cas_root.display()));
    }

    let migration = run_project_phase("user-level migration", false, || {
        run_schema_migrations(args, cli, Some(&cas_root))
    });
    if migration.failed() {
        return migration;
    }
    match sync_user_builtins(cli) {
        Ok(()) => ProjectPhase::Ok(format!(
            "migrated {} and synced builtins",
            cas_root.display()
        )),
        Err(error) => ProjectPhase::Failed(error.to_string()),
    }
}

/// Repair the pre-cas-bc42 Tantivy root as part of the native update walk.
///
/// Search repair is deliberately advisory: a held lock, malformed legacy
/// index, or unavailable project store must be visible to the operator but
/// must not prevent the remaining update phases or other projects from being
/// refreshed. The cheap metadata check avoids opening a project store on the
/// common no-stray-root path.
fn repair_project_search_index(cas_root: &Path, dry_run: bool, cli: &Cli) -> ProjectPhase {
    let legacy_dir = cas_root.join("index");
    let has_legacy_root = legacy_dir.join("meta.json").is_file();
    let has_resumable_sweep = legacy_dir.join(".managed.json").is_file();

    let phase = if dry_run {
        ProjectPhase::Planned("legacy-root repair".to_string())
    } else if !has_legacy_root && !has_resumable_sweep {
        ProjectPhase::Ok("no stray root".to_string())
    } else {
        match open_store(cas_root) {
            Err(error) => {
                ProjectPhase::Warning(format!("could not open project store for repair: {error}"))
            }
            Ok(store) => match repair_legacy_index_bounded(
                cas_root,
                store,
                LegacyRepairLimits::default(),
                UPDATE_REPAIR_BUDGET,
            ) {
                Ok(LegacyRepairOutcome::NoLegacyRoot) => {
                    ProjectPhase::Ok("no stray root".to_string())
                }
                Ok(LegacyRepairOutcome::Busy { reason }) => ProjectPhase::Warning(format!(
                    "busy, will retry in the daemon cycle ({reason})"
                )),
                Ok(LegacyRepairOutcome::Repaired(repair)) => {
                    let mut detail = format!(
                        "repaired {} stranded memories ({} re-queued)",
                        repair.legacy_documents, repair.requeued_entries
                    );
                    if !repair.errors.is_empty() {
                        detail.push_str(&format!(
                            "; {} memory(s) remain queued for retry",
                            repair.errors.len()
                        ));
                    }
                    if !repair.unswept_files.is_empty() {
                        detail.push_str(&format!(
                            "; {} legacy file(s) remain for the next sweep",
                            repair.unswept_files.len()
                        ));
                    }
                    if !repair.retired_non_entry_documents.is_empty() {
                        detail.push_str(&format!(
                            "; retired non-memory documents: {}",
                            repair
                                .retired_non_entry_documents
                                .iter()
                                .map(|(kind, count)| format!("{count} {kind}"))
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                    if repair.errors.is_empty()
                        && repair.unswept_files.is_empty()
                        && repair.retired_non_entry_documents.is_empty()
                    {
                        ProjectPhase::Ok(detail)
                    } else {
                        ProjectPhase::Warning(detail)
                    }
                }
                Err(error) => ProjectPhase::Warning(format!("repair failed: {error}")),
            },
        }
    };

    if !cli.json {
        let (status, detail) = match &phase {
            ProjectPhase::Ok(detail) => ("OK", detail),
            ProjectPhase::Planned(detail) => ("DRY RUN", detail),
            ProjectPhase::Warning(detail) => ("WARN", detail),
            ProjectPhase::Skipped(detail) => ("SKIP", detail),
            ProjectPhase::Failed(detail) => ("FAIL", detail),
        };
        println!("    [{status}] search index: {detail}");
    }

    phase
}

fn run_project_phase(
    name: &str,
    dry_run: bool,
    operation: impl FnOnce() -> anyhow::Result<()>,
) -> ProjectPhase {
    if dry_run {
        return ProjectPhase::Planned(name.to_string());
    }
    match operation() {
        Ok(()) => ProjectPhase::Ok(name.to_string()),
        Err(error) => ProjectPhase::Failed(format!("{name}: {error:#}")),
    }
}

/// Refresh the user-level membership cache and resolve the project scope
/// without changing an explicit project team selection or canonical project
/// pin. `maybe_adopt_team_scope` already carries that precedence contract.
fn refresh_project_membership(cas_root: &Path, dry_run: bool) -> ProjectPhase {
    if !cas_root.join("cloud.json").exists() {
        return ProjectPhase::Skipped("not cloud-linked".to_string());
    }

    let user = match CloudConfig::load_user() {
        Ok(config) => config,
        Err(error) => return ProjectPhase::Failed(format!("could not read login: {error}")),
    };
    let project = match CloudConfig::load_from_cas_dir(cas_root) {
        Ok(config) => config,
        Err(error) => {
            return ProjectPhase::Failed(format!("could not read project cloud config: {error}"));
        }
    };
    let token = user.token.as_deref().or(project.token.as_deref());
    let Some(token) = token else {
        return ProjectPhase::Skipped("not logged in — run cas login".to_string());
    };

    if dry_run {
        return ProjectPhase::Planned("refresh memberships and validate selected team".to_string());
    }

    match fetch_and_cache_teams(&project.endpoint, token) {
        FetchTeamsOutcome::Updated { team_count } => match maybe_adopt_team_scope(cas_root) {
            Ok(adoption) => ProjectPhase::Ok(format!(
                "refreshed {team_count} membership(s); {}",
                adoption_summary(&adoption)
            )),
            Err(error) => ProjectPhase::Failed(format!(
                "memberships refreshed but project scope could not be validated: {error}"
            )),
        },
        FetchTeamsOutcome::Empty => ProjectPhase::Ok(
            "no team selected — run cas cloud team set if this project should use team scope"
                .to_string(),
        ),
        FetchTeamsOutcome::AuthFailed => {
            ProjectPhase::Failed("credentials rejected — run cas login".to_string())
        }
        FetchTeamsOutcome::NetworkError(error) => {
            ProjectPhase::Failed(format!("membership refresh failed: {error}"))
        }
    }
}

fn adoption_summary(adoption: &crate::cloud::TeamScopeAdoption) -> &'static str {
    match adoption {
        crate::cloud::TeamScopeAdoption::Adopted(_) => "adopted team scope",
        crate::cloud::TeamScopeAdoption::AlreadyScoped { .. } => "already scoped",
        crate::cloud::TeamScopeAdoption::OptedOut => "opted out",
        crate::cloud::TeamScopeAdoption::NotLoggedIn => "not logged in",
        crate::cloud::TeamScopeAdoption::NoResolvableTeam { .. } => "no resolvable team",
    }
}

fn sync_project_cloud(
    cas_root: &Path,
    dry_run: bool,
    cli: &Cli,
) -> (ProjectPhase, Vec<SyncSummary>) {
    if !cas_root.join("cloud.json").exists() {
        return (
            ProjectPhase::Skipped("not cloud-linked".to_string()),
            Vec::new(),
        );
    }
    let config = match CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root) {
        Ok(config) => config,
        Err(error) => {
            return (
                ProjectPhase::Failed(format!("could not read cloud config: {error}")),
                Vec::new(),
            );
        }
    };
    if !config.is_logged_in() {
        return (
            ProjectPhase::Skipped("not logged in — run cas login".to_string()),
            Vec::new(),
        );
    }
    if dry_run {
        return (ProjectPhase::Planned("cloud sync".to_string()), Vec::new());
    }
    match execute_sync_with_summaries(
        &CloudSyncArgs {
            dry_run: false,
            full: false,
            rehome: false,
        },
        cli,
        cas_root,
    ) {
        Ok(summaries) => {
            let phase = cloud_phase_from_summaries(&summaries);
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
            if let Err(error) = summaries
                .iter()
                .try_for_each(|summary| render_sync_summary(&mut fmt, summary, cli.verbose))
            {
                return (
                    ProjectPhase::Failed(format!("could not render cloud summary: {error}")),
                    summaries,
                );
            }
            (phase, summaries)
        }
        Err(error) => (
            ProjectPhase::Failed(format!("cloud sync: {error:#}")),
            Vec::new(),
        ),
    }
}

fn cloud_phase_from_summaries(summaries: &[SyncSummary]) -> ProjectPhase {
    let failed = summaries.iter().any(|summary| {
        !summary.errors.is_empty()
            || summary.failed > 0
            || summary.pending > 0
            || summary.team_backlog_pending > 0
            || summary.team_backlog_failed > 0
    });
    let mut parts = Vec::new();
    for summary in summaries {
        if summary.is_push() {
            let pushed = summary.counts.values().sum::<usize>();
            if pushed > 0 {
                parts.push(format!("{pushed} pushed"));
            }
            let queued = summary.pending
                + summary.failed
                + summary.team_backlog_pending
                + summary.team_backlog_failed;
            if queued > 0 {
                parts.push(format!("{queued} queued"));
            }
        } else if summary.is_pull() {
            let pulled = summary.counts.values().sum::<usize>();
            if pulled > 0 {
                parts.push(format!("{pulled} pulled"));
            }
        }
        if summary.knowledge_pushed > 0
            || summary.knowledge_pulled > 0
            || summary.knowledge_embedded > 0
        {
            parts.push(format!(
                "knowledge {} pushed, {} pulled, {} embedded",
                summary.knowledge_pushed, summary.knowledge_pulled, summary.knowledge_embedded
            ));
        }
    }
    if parts.is_empty() {
        parts.push("up to date".to_string());
    }
    let detail = parts.join("; ");
    if failed {
        ProjectPhase::Warning(detail)
    } else {
        ProjectPhase::Ok(detail)
    }
}

fn print_project_refresh_summary(
    receipts: &[ProjectRefreshReceipt],
    user_level: &ProjectPhase,
    user_details: &str,
    skipped_unregistered: &[PathBuf],
    cli: &Cli,
) {
    if cli.json {
        let projects = receipts
            .iter()
            .map(|receipt| {
                serde_json::json!({
                    "project": receipt.project,
                    "store": receipt.project.join(".cas"),
                    "registered": !receipt.unregistered,
                    "migration": receipt.migration.summary(),
                    "search_index": receipt.search_index.summary(),
                    "skills": receipt.skills.summary(),
                    "membership": receipt.membership.summary(),
                    "cloud_sync": receipt.cloud.summary(),
                })
            })
            .collect::<Vec<_>>();
        println!(
            "{}",
            serde_json::json!({
                "projects": projects,
                "user_level_store": {
                    "store": user_level_store_root(),
                    "status": user_level.summary(),
                },
                // Retained for compatibility with readers of the pre-cas-9d5c
                // receipt, which only ever reported the builtin distribution.
                "user_builtins": user_level.summary(),
                "skipped_unregistered": skipped_unregistered,
            })
        );
        return;
    }

    let mut warnings = RepeatedWarningCollector::default();
    let mut details = Vec::with_capacity(receipts.len());
    for receipt in receipts {
        let project = project_display_name(&receipt.project, cli.verbose);
        // Collect warnings from every phase, including successful phases whose
        // output is intentionally omitted from the compact transcript.
        warnings.collect_output(&project, &receipt.details);
        let detail = render_project_phase_details(receipt, cli.verbose, &mut warnings, &project);
        let show_detail = cli.verbose
            || [
                &receipt.migration,
                &receipt.search_index,
                &receipt.skills,
                &receipt.membership,
                &receipt.cloud,
            ]
            .into_iter()
            .any(|phase| !phase.is_ok());
        details.push((project, show_detail.then_some(detail)));
    }
    if !user_details.is_empty() {
        let project = "user-level";
        let detail =
            strip_repeated_warning_lines(user_details, &mut warnings, project, cli.verbose, false);
        if cli.verbose && !detail.trim().is_empty() {
            details.push((project.to_owned(), Some(detail)));
        }
    }

    let table_lines = render_project_table_plain(receipts, cli.verbose);
    let mut table_lines = table_lines.lines();
    if let Some(header) = table_lines.next() {
        println!("{header}");
    }
    for ((_, row), (project, detail)) in receipts.iter().zip(table_lines).zip(details) {
        println!("{row}");
        if let Some(detail) = detail
            && !detail.trim().is_empty()
        {
            println!("  {project} details:");
            for line in detail.lines().filter(|line| !line.trim().is_empty()) {
                println!("    {line}");
            }
        }
    }

    // The user-level store is host state, so it gets a named line of its own
    // rather than a project row it would otherwise be miscounted in.
    println!(
        "  [{}] user-level store: {}",
        user_level.status_label().trim_matches(['[', ']']),
        user_level.detail()
    );

    if !skipped_unregistered.is_empty() {
        println!(
            "  not refreshed (unregistered): {} — run `cas update` inside them or `cas update --register <path>`",
            skipped_unregistered.len()
        );
        for path in skipped_unregistered.iter().take(5) {
            println!("    ! {}", path.display());
        }
        if skipped_unregistered.len() > 5 {
            println!("    … and {} more", skipped_unregistered.len() - 5);
        }
    }
    print!("{}", warnings.render(cli.verbose));
}

/// How far below each scan root discovery will walk. Deep enough for the
/// `~/Workspace/group/project` shapes real hosts use, shallow enough that a
/// whole-home walk stays bounded on a machine with a large source tree.
const MAX_SCAN_DEPTH: usize = 8;

/// What one discovery pass found, split by what the refresh can actually do
/// with each entry.
#[derive(Debug, Default)]
struct ProjectDiscovery {
    /// Every project the refresh will process: the host registry plus every
    /// scan-discovered directory that actually holds a store.
    projects: Vec<PathBuf>,
    /// The subset of `projects` that the filesystem scan found but the host
    /// registry does not know about. These are refreshed and then registered,
    /// so discovery converges without the operator doing anything.
    unregistered: BTreeSet<PathBuf>,
    /// Scan-discovered directories carrying a `.cas/` but no `cas.db`. There
    /// is nothing to migrate, so they are reported rather than refreshed —
    /// previously they were silently absent from the receipt entirely.
    skipped_unregistered: Vec<PathBuf>,
}

/// The host's user-level store (`~/.cas`). It is never a project — it gets its
/// own refresh phase — but it must be named, because before cas-9d5c it was
/// neither refreshed nor mentioned.
fn user_level_store_root() -> Option<PathBuf> {
    dirs::home_dir().map(|home| canonical_path(&home.join(".cas")))
}

/// Discovery is the union of the host's known-repo registry and a bounded
/// filesystem scan. The scan is not a fallback for binary-only machines: the
/// registry only holds repos some Cassy entry point happened to touch, so a
/// project cloned and initialized by hand is invisible to it forever.
///
/// The scan must keep descending after it records a project. `~/.cas` exists on
/// every host that has ever run Cassy, and the previous early return meant the
/// walk recorded `$HOME`, stopped, and then dropped `$HOME` as host state — a
/// scan that structurally could not discover anything (cas-9d5c). The same
/// early return also hid every project nested under a parent that is itself a
/// project, which is the normal monorepo-of-repos layout.
fn discover_local_projects(current_cas_root: Option<&Path>) -> ProjectDiscovery {
    let mut registered = BTreeSet::new();
    if let Ok(known) = crate::worktree::discovery::list_tracked_repos() {
        for repo in known.into_iter().filter(|repo| repo.healthy) {
            registered.insert(canonical_path(&repo.path));
        }
    }

    let roots = std::env::var_os("CAS_PROJECT_ROOTS")
        .map(|raw| {
            raw.to_string_lossy()
                .split(',')
                .filter(|root| !root.trim().is_empty())
                .map(|root| PathBuf::from(root.trim()))
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| dirs::home_dir().into_iter().collect());
    let mut scanned = BTreeSet::new();
    for root in roots {
        scan_for_projects(&root, MAX_SCAN_DEPTH, &mut scanned);
    }

    let user_level_root = user_level_store_root();
    let home = dirs::home_dir().map(|home| canonical_path(&home));

    let mut projects = registered.clone();
    let mut unregistered = BTreeSet::new();
    let mut skipped_unregistered = Vec::new();
    for candidate in scanned {
        if Some(&candidate) == home.as_ref() {
            continue;
        }
        if registered.contains(&candidate) {
            continue;
        }
        if candidate.join(".cas").join("cas.db").is_file() {
            projects.insert(candidate.clone());
            unregistered.insert(candidate);
        } else {
            skipped_unregistered.push(candidate);
        }
    }

    // The store the operator is standing in always counts, even if neither the
    // registry nor the scan roots cover it.
    if let Some(cas_root) = current_cas_root
        && let Some(project) = cas_root.parent()
        && user_level_root.as_deref() != Some(canonical_path(cas_root).as_path())
    {
        projects.insert(canonical_path(project));
    }

    // `~/.cas` is host state, not a project: it is migrated by its own
    // user-level phase so it can never be counted in the project totals.
    if let Some(home) = home {
        projects.remove(&home);
        unregistered.remove(&home);
    }

    ProjectDiscovery {
        projects: projects.into_iter().collect(),
        unregistered,
        skipped_unregistered,
    }
}

fn canonical_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

/// Record `root` when it carries a `.cas/`, then keep walking its children.
///
/// Never descends into a `.cas/` directory itself: factory worktrees
/// (`.cas/worktrees/<lane>`) and migration backups (`.cas/backup/<stamp>`) both
/// contain a `cas.db` and are emphatically not separate projects. Hidden
/// directories are skipped for the same reason plus cost — `~/.cargo`,
/// `~/.rustup`, and `~/.claude` are large and hold no projects, and anything
/// genuinely living under one is reachable through the host registry.
fn scan_for_projects(root: &Path, depth: usize, projects: &mut BTreeSet<PathBuf>) {
    if root.join(".cas").is_dir() {
        projects.insert(canonical_path(root));
    }
    if depth == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = entry.file_type() else {
            continue;
        };
        if !metadata.is_dir() || metadata.is_symlink() {
            continue;
        }
        let name = path.file_name().and_then(|name| name.to_str());
        if name.is_none_or(|name| name.starts_with('.')) {
            continue;
        }
        if matches!(
            name,
            Some(
                "node_modules"
                    | "target"
                    | "venv"
                    | "dist"
                    | "build"
                    | "vendor"
                    | "Library"
                    | "snap"
            )
        ) {
            continue;
        }
        scan_for_projects(&path, depth - 1, projects);
    }
}

/// Sync rules, skills, and configuration to .claude/.codex directories
fn sync_claude_files(cli: &Cli, cas_root_param: Option<&Path>) -> anyhow::Result<()> {
    // cas_root is optional - if not provided and Cassy is not initialized, nothing to sync
    let cas_root = match cas_root_param {
        Some(path) => path.to_path_buf(),
        None => {
            // Not initialized, nothing to sync
            return Ok(());
        }
    };

    let project_root = cas_root.parent().unwrap_or(&cas_root);
    let claude_dir = project_root.join(".claude");
    let codex_dir = project_root.join(".codex");
    let codex_enabled = codex_dir.exists();
    let grok_dir = project_root.join(".grok");
    let grok_enabled = grok_dir.exists();

    let theme = ActiveTheme::default();

    // Builtin files are generated into consumer projects and should not be
    // committed there. The authoring checkout is exempted inside the helper
    // because its rendered files are intentional tracked fixtures.
    let mut builtin_harnesses = vec![cas_mux::SupervisorCli::Claude];
    if codex_enabled {
        builtin_harnesses.push(cas_mux::SupervisorCli::Codex);
    }
    if grok_enabled {
        builtin_harnesses.push(cas_mux::SupervisorCli::Grok);
    }
    let builtin_gitignore = ensure_builtin_gitignore(project_root, &builtin_harnesses)?;

    if !cli.json {
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme.clone());
        fmt.subheading("Syncing .claude files")?;
        if builtin_gitignore.updated {
            fmt.write_raw("  ")?;
            fmt.success("Updated .gitignore with Cassy-managed builtin paths")?;
        }
        if !builtin_gitignore.tracked_paths.is_empty() {
            fmt.write_raw("  ")?;
            fmt.warning(&format!(
                "{} Cassy-managed builtin path(s) are already tracked and will not be hidden by \
                 .gitignore:",
                builtin_gitignore.tracked_paths.len()
            ))?;
            for path in &builtin_gitignore.tracked_paths {
                fmt.write_raw(&format!("    ! {path}"))?;
                fmt.newline()?;
            }
            fmt.write_raw("    ")?;
            fmt.write_raw(
                "To make the ignore rule effective, review and run `git rm --cached <path>` \
                 for each path, then commit the removal.",
            )?;
            fmt.newline()?;
        }
    }

    // Track what was updated for JSON output
    let mut config_updated = Vec::new();
    let mut codex_config_updated = Vec::new();

    // Update configuration files
    // 1. Claude Code hooks (.claude/settings.json)
    match configure_claude_hooks(project_root, false) {
        Ok(true) => {
            config_updated.push("settings.json");
            if !cli.json {
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme.clone());
                fmt.write_raw("  ")?;
                fmt.success("Updated .claude/settings.json")?;
            }
        }
        Ok(false) => {} // No changes needed
        Err(e) => {
            if !cli.json {
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme.clone());
                fmt.write_raw("  ")?;
                fmt.warning(&format!("Could not update settings.json: {e}"))?;
            }
        }
    }

    // 2. MCP server configuration (.mcp.json)
    match configure_mcp_server(project_root) {
        Ok(true) => {
            config_updated.push(".mcp.json");
            if !cli.json {
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme.clone());
                fmt.write_raw("  ")?;
                fmt.success("Updated .mcp.json")?;
            }
        }
        Ok(false) => {} // No changes needed
        Err(e) => {
            if !cli.json {
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme.clone());
                fmt.write_raw("  ")?;
                fmt.warning(&format!("Could not update .mcp.json: {e}"))?;
            }
        }
    }

    // 3. CLAUDE.md directive
    match update_claude_md(project_root) {
        Ok(true) => {
            config_updated.push("CLAUDE.md");
            if !cli.json {
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme.clone());
                fmt.write_raw("  ")?;
                fmt.success("Updated CLAUDE.md")?;
            }
        }
        Ok(false) => {} // No changes needed
        Err(e) => {
            if !cli.json {
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme.clone());
                fmt.write_raw("  ")?;
                fmt.warning(&format!("Could not update CLAUDE.md: {e}"))?;
            }
        }
    }

    // 4. Main Cassy skill (.claude/skills/cas/SKILL.md)
    match generate_cas_skill(project_root) {
        Ok(true) => {
            config_updated.push("skills/cas/SKILL.md");
            if !cli.json {
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme.clone());
                fmt.write_raw("  ")?;
                fmt.success("Updated .claude/skills/cas/SKILL.md")?;
            }
        }
        Ok(false) => {} // No changes needed or user-customized
        Err(e) => {
            if !cli.json {
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme.clone());
                fmt.write_raw("  ")?;
                fmt.warning(&format!("Could not update Cassy skill: {e}"))?;
            }
        }
    }

    // Sync database rules
    let rule_store = open_rule_store(&cas_root)?;
    let rules = rule_store.list()?;
    let rule_syncer = Syncer::with_defaults(project_root);
    let rule_report = rule_syncer.sync_all(&rules)?;

    // Capture explicit reference deletions before database skill sync can
    // rehydrate an older stored copy. Builtin sync consumes these one-shot
    // markers below and installs the current embedded reference.
    mark_missing_owned_references_for_replacement(cas_mux::SupervisorCli::Claude, &claude_dir)?;
    if codex_enabled {
        mark_missing_owned_references_for_replacement(cas_mux::SupervisorCli::Codex, &codex_dir)?;
    }
    if grok_enabled {
        mark_missing_owned_references_for_replacement(cas_mux::SupervisorCli::Grok, &grok_dir)?;
    }

    // Sync database skills (this may remove stale dirs)
    let skill_store = open_skill_store(&cas_root)?;
    let skills = skill_store.list(None)?;
    let skill_syncer = SkillSyncer::with_defaults(project_root);
    let skill_report = skill_syncer.sync_all(&skills)?;

    // Sync built-in agents, skills, and commands AFTER database sync
    // (so they don't get removed as "stale" by the skill syncer)
    let builtin_result =
        sync_all_builtins_for_project(cas_mux::SupervisorCli::Claude, project_root)?;
    if !cli.json {
        report_builtin_sync(&builtin_result, ".claude", &theme)?;
    }

    // After all skill writes complete, refresh the skill-sync sentinel so that
    // live sessions can detect the new content and emit `reloadSkills: true`
    // on their next SessionStart (cas-f9ad). Best-effort: failure to write the
    // sentinel is non-fatal; the hot-reload feature simply won't fire.
    write_skill_sync_sentinel(&cas_root);

    // Sync factory tooling helper templates.
    let factory_tooling_result = match factory_tooling::setup_factory_tooling(project_root) {
        Ok(summary) => {
            if !cli.json && !summary.is_empty() {
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme.clone());
                fmt.write_raw("  ")?;
                fmt.success(&format!("Factory tooling: {summary}"))?;
            }
            summary
        }
        Err(e) => {
            if !cli.json {
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme.clone());
                fmt.write_raw("  ")?;
                fmt.warning(&format!("Could not update factory tooling: {e}"))?;
            }
            String::new()
        }
    };

    // Codex config + built-ins
    let mut codex_modified_references = 0;
    let codex_builtins_updated = if codex_enabled {
        if !cli.json {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme.clone());
            fmt.subheading("Syncing .codex files")?;
        }

        match provision_codex_project(project_root) {
            Ok(true) => {
                codex_config_updated.push("config.toml");
                codex_config_updated.push("hooks.json");
                if !cli.json {
                    let mut out = io::stdout();
                    let mut fmt = Formatter::stdout(&mut out, theme.clone());
                    fmt.write_raw("  ")?;
                    fmt.success("Updated .codex/config.toml and .codex/hooks.json")?;
                    fmt.write_raw("  ")?;
                    fmt.warning("Review the new Codex hook with /hooks before it can run")?;
                }
            }
            Ok(false) => {} // No changes needed
            Err(e) => {
                if !cli.json {
                    let mut out = io::stdout();
                    let mut fmt = Formatter::stdout(&mut out, theme.clone());
                    fmt.write_raw("  ")?;
                    fmt.warning(&format!("Could not update config.toml: {e}"))?;
                }
            }
        }

        let codex_result =
            sync_all_builtins_for_project(cas_mux::SupervisorCli::Codex, project_root)?;
        codex_modified_references = codex_result.modified_reference_files.len();
        if !cli.json {
            report_builtin_sync(&codex_result, ".codex", &theme)?;
        }
        codex_result.total_updated()
    } else {
        0
    };

    // Grok built-ins (EPIC cas-8888, Phase 5). No separate config writer:
    // `.mcp.json` is already kept current by the unconditional
    // configure_mcp_server call above, which Grok reads directly.
    let mut grok_modified_references = 0;
    let grok_builtins_updated = if grok_enabled {
        if !cli.json {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme.clone());
            fmt.subheading("Syncing .grok files")?;
        }

        let grok_result =
            sync_all_builtins_for_project(cas_mux::SupervisorCli::Grok, project_root)?;
        grok_modified_references = grok_result.modified_reference_files.len();
        if !cli.json {
            report_builtin_sync(&grok_result, ".grok", &theme)?;
        }
        grok_result.total_updated()
    } else {
        0
    };

    if cli.json {
        let config_json: Vec<String> = config_updated.iter().map(|s| format!("\"{s}\"")).collect();
        let codex_config_json: Vec<String> = codex_config_updated
            .iter()
            .map(|s| format!("\"{s}\""))
            .collect();
        println!(
            r#"{{"config_updated":[{}],"builtins_updated":{},"builtin_reference_conflicts":{},"codex_config_updated":[{}],"codex_builtins_updated":{},"codex_builtin_reference_conflicts":{},"grok_builtins_updated":{},"grok_builtin_reference_conflicts":{},"rules_synced":{},"rules_removed":{},"skills_synced":{},"skills_removed":{},"factory_tooling":"{}","builtin_gitignore_updated":{},"builtin_gitignore_tracked":{}}}"#,
            config_json.join(","),
            builtin_result.total_updated(),
            builtin_result.modified_reference_files.len(),
            codex_config_json.join(","),
            codex_builtins_updated,
            codex_modified_references,
            grok_builtins_updated,
            grok_modified_references,
            rule_report.synced,
            rule_report.removed,
            skill_report.synced,
            skill_report.removed,
            factory_tooling_result,
            builtin_gitignore.updated,
            serde_json::to_string(&builtin_gitignore.tracked_paths)?
        );
    } else {
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);

        // Built-in sync results are reported per harness, inline under each
        // `Syncing <dir> files` heading (cas-27bf) — see report_builtin_sync.

        // Report database rule sync
        if rule_report.synced > 0 || rule_report.removed > 0 {
            fmt.write_raw("  ")?;
            fmt.success(&format!(
                "Synced {} rules, removed {}",
                rule_report.synced, rule_report.removed
            ))?;
        } else {
            fmt.write_raw("  ")?;
            fmt.success("Database rules up to date")?;
        }

        // Report database skill sync
        if skill_report.synced > 0 || skill_report.removed > 0 {
            fmt.write_raw("  ")?;
            fmt.success(&format!(
                "Synced {} skills, removed {}",
                skill_report.synced, skill_report.removed
            ))?;
        } else {
            fmt.write_raw("  ")?;
            fmt.success("Database skills up to date")?;
        }
    }

    Ok(())
}

/// Distribute embedded built-in skills/agents/commands to user-level dirs
/// (`~/.claude` for Claude Code, `~/.codex` for Codex if present).
///
/// Why this exists: factory worker worktrees that don't ship `.claude/skills/`
/// in their tracked tree fall back to user-level skills. Without a user-level
/// refresh path, those workers silently consume stale skill prompts after a
/// `cas update` because `--sync` only writes into the current project. This is
/// the user-level analogue of `--sync`: it refreshes builtins and, for an
/// existing Codex install, its global MCP configuration and trusted hook state.
/// It never writes project-scoped settings, CLAUDE.md, or db-backed rules/skills.
fn sync_user_builtins(cli: &Cli) -> anyhow::Result<()> {
    let home = dirs::home_dir().ok_or_else(|| {
        anyhow::anyhow!("could not resolve user home directory; set $HOME and retry")
    })?;
    let claude_dir = home.join(".claude");
    let codex_dir = home.join(".codex");
    let grok_dir = home.join(".grok");

    let theme = ActiveTheme::default();

    if !cli.json {
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme.clone());
        fmt.subheading("Distributing built-ins to user-level")?;
    }

    // Claude: gated on dir existence — if a user has no ~/.claude, they're
    // not using Claude Code globally and we don't want to materialize an
    // empty dir for them.
    let mut claude_pruned: Vec<String> = Vec::new();
    let mut codex_pruned: Vec<String> = Vec::new();
    let mut grok_pruned: Vec<String> = Vec::new();

    let claude_result = if claude_dir.exists() {
        let r = sync_all_builtins_for_harness(cas_mux::SupervisorCli::Claude, &claude_dir)?;
        // Prune legacy non-managed cas-* orphans (e.g. cas-playwright-debug) the
        // project-level sync already drops but the user-level path historically
        // never did (cas-e0d1).
        claude_pruned =
            prune_stale_user_skills_for_harness(cas_mux::SupervisorCli::Claude, &claude_dir)?;
        if !cli.json {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme.clone());
            fmt.write_raw("  ")?;
            if r.total_updated() > 0 {
                fmt.success(&format!(
                    "~/.claude: updated {} files ({} agents, {} skills)",
                    r.total_updated(),
                    r.agents_updated,
                    r.skills_updated
                ))?;
                for file in &r.updated_files {
                    fmt.write_raw(&format!("    + {file}"))?;
                    fmt.newline()?;
                }
            } else {
                fmt.success("~/.claude: built-ins up to date")?;
            }
            for name in &claude_pruned {
                fmt.write_raw(&format!("    - skills/{name} (removed stale orphan)"))?;
                fmt.newline()?;
            }
            drop(fmt);
            report_modified_builtin_references(&r, "~/.claude", &theme)?;
        }
        Some(r)
    } else {
        if !cli.json {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme.clone());
            fmt.write_raw("  ")?;
            fmt.warning("~/.claude does not exist — skipping (Claude Code not installed?)")?;
        }
        None
    };

    let codex_result = if codex_dir.exists() {
        let codex_config_updated = provision_codex_user_config(&codex_dir)?;
        let r = sync_all_builtins_for_harness(cas_mux::SupervisorCli::Codex, &codex_dir)?;
        codex_pruned =
            prune_stale_user_skills_for_harness(cas_mux::SupervisorCli::Codex, &codex_dir)?;
        if !cli.json {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme.clone());
            fmt.write_raw("  ")?;
            if r.total_updated() > 0 || codex_config_updated {
                fmt.success(&format!(
                    "~/.codex: updated {} files ({} agents, {} skills; config/hooks refreshed)",
                    r.total_updated() + usize::from(codex_config_updated),
                    r.agents_updated,
                    r.skills_updated
                ))?;
            } else {
                fmt.success("~/.codex: built-ins up to date")?;
            }
            for name in &codex_pruned {
                fmt.write_raw(&format!("    - skills/{name} (removed stale orphan)"))?;
                fmt.newline()?;
            }
            drop(fmt);
            report_modified_builtin_references(&r, "~/.codex", &theme)?;
        }
        Some(r)
    } else {
        // No nag for absent ~/.codex — Codex is opt-in and most users won't
        // have it. Silent skip.
        None
    };

    let grok_result = if grok_dir.exists() {
        let r = sync_all_builtins_for_harness(cas_mux::SupervisorCli::Grok, &grok_dir)?;
        grok_pruned = prune_stale_user_skills_for_harness(cas_mux::SupervisorCli::Grok, &grok_dir)?;
        if !cli.json {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme.clone());
            fmt.write_raw("  ")?;
            if r.total_updated() > 0 {
                fmt.success(&format!(
                    "~/.grok: updated {} files ({} agents, {} skills)",
                    r.total_updated(),
                    r.agents_updated,
                    r.skills_updated
                ))?;
            } else {
                fmt.success("~/.grok: built-ins up to date")?;
            }
            for name in &grok_pruned {
                fmt.write_raw(&format!("    - skills/{name} (removed stale orphan)"))?;
                fmt.newline()?;
            }
            drop(fmt);
            report_modified_builtin_references(&r, "~/.grok", &theme)?;
        }
        Some(r)
    } else {
        // No nag for absent ~/.grok — Grok is opt-in and most users won't
        // have it. Silent skip.
        None
    };

    if cli.json {
        let claude_total = claude_result
            .as_ref()
            .map(|r| r.total_updated())
            .unwrap_or(0);
        let codex_total = codex_result
            .as_ref()
            .map(|r| r.total_updated())
            .unwrap_or(0);
        let grok_total = grok_result.as_ref().map(|r| r.total_updated()).unwrap_or(0);
        let claude_present = claude_dir.exists();
        let codex_present = codex_dir.exists();
        let grok_present = grok_dir.exists();
        let claude_pruned_n = claude_pruned.len();
        let codex_pruned_n = codex_pruned.len();
        let grok_pruned_n = grok_pruned.len();
        let claude_conflicts = claude_result
            .as_ref()
            .map(|r| r.modified_reference_files.len())
            .unwrap_or(0);
        let codex_conflicts = codex_result
            .as_ref()
            .map(|r| r.modified_reference_files.len())
            .unwrap_or(0);
        let grok_conflicts = grok_result
            .as_ref()
            .map(|r| r.modified_reference_files.len())
            .unwrap_or(0);
        println!(
            r#"{{"claude_present":{claude_present},"claude_builtins_updated":{claude_total},"claude_builtin_reference_conflicts":{claude_conflicts},"claude_skills_pruned":{claude_pruned_n},"codex_present":{codex_present},"codex_builtins_updated":{codex_total},"codex_builtin_reference_conflicts":{codex_conflicts},"codex_skills_pruned":{codex_pruned_n},"grok_present":{grok_present},"grok_builtins_updated":{grok_total},"grok_builtin_reference_conflicts":{grok_conflicts},"grok_skills_pruned":{grok_pruned_n}}}"#
        );
    }

    Ok(())
}

/// Prefix every `schema_status` receipt with the store it actually migrated.
///
/// Without this a `CAS_ROOT` override, a user-level phase, and a project phase
/// all emit byte-identical lines, so an operator reading the receipt cannot
/// tell which store moved — the exact ambiguity behind the "says applied but
/// didn't" report in cas-9d5c.
fn schema_status_json(store: Option<&Path>, body: &str) -> String {
    let store = match store {
        Some(path) => serde_json::Value::String(path.display().to_string()),
        None => serde_json::Value::Null,
    };
    format!(r#"{{"store":{store},{body}}}"#)
}

/// Run schema migrations only
fn run_schema_migrations(
    args: &UpdateArgs,
    cli: &Cli,
    cas_root_param: Option<&Path>,
) -> anyhow::Result<()> {
    // cas_root is optional - if not provided, Cassy is not initialized
    let cas_root = match cas_root_param {
        Some(path) => path.to_path_buf(),
        None => {
            if cli.json {
                println!(
                    "{}",
                    schema_status_json(
                        None,
                        r#""schema_status":"not_initialized","migrations_applied":0"#
                    )
                );
            } else {
                let mut out = io::stdout();
                let theme = ActiveTheme::default();
                let mut fmt = Formatter::stdout(&mut out, theme);
                fmt.warning("Cassy not initialized in this directory")?;
                fmt.write_raw("  Run ")?;
                fmt.write_accent("cas init")?;
                fmt.write_raw(" to initialize")?;
                fmt.newline()?;
            }
            return Ok(());
        }
    };

    let project_root = cas_root.parent().unwrap_or(&cas_root);

    // Verify the database is initialized before attempting migrations
    let db_path = cas_root.join("cas.db");
    if db_path.exists() {
        let conn = rusqlite::Connection::open(&db_path)?;
        let table_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('entries', 'rules', 'tasks')",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        if table_count < 3 {
            if cli.json {
                println!(
                    "{}",
                    schema_status_json(
                        Some(&cas_root),
                        r#""schema_status":"not_initialized","migrations_applied":0"#
                    )
                );
            } else {
                let mut out = io::stdout();
                let theme = ActiveTheme::default();
                let mut fmt = Formatter::stdout(&mut out, theme);
                fmt.warning("Cassy database not initialized")?;
                fmt.write_raw("  Run ")?;
                fmt.write_accent("cas init")?;
                fmt.write_raw(" to initialize")?;
                fmt.newline()?;
            }
            return Ok(());
        }
    }

    let status = check_migrations(&cas_root)?;

    // Build transaction with all pending changes
    let tx = build_update_transaction(project_root, &cas_root, &status, args.keep_backup);

    if args.dry_run {
        let claude_dir = project_root.join(".claude");
        let codex_dir = project_root.join(".codex");
        return show_enhanced_dry_run(&tx, &status, &claude_dir, &codex_dir, cli);
    }

    if !tx.has_changes() {
        if cli.json {
            println!(
                "{}",
                schema_status_json(
                    Some(&cas_root),
                    &format!(
                        r#""schema_status":"up_to_date","current_version":{},"migrations_applied":0"#,
                        status.current_version
                    )
                )
            );
        } else {
            let mut out = io::stdout();
            let theme = ActiveTheme::default();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.success(&format!(
                "Schema up to date (v{}) — {}",
                status.current_version,
                cas_root.display()
            ))?;
        }
        return Ok(());
    }

    // Create backup before making changes
    let mut tx = tx;
    if !cli.json {
        let mut out = io::stdout();
        let theme = ActiveTheme::default();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.write_accent("\u{2192} ")?;
        fmt.write_raw("Creating backup...")?;
        fmt.newline()?;
    }
    tx.backup()?;
    if let Some(backup_dir) = tx.backup_dir() {
        if !cli.json {
            let mut out = io::stdout();
            let theme = ActiveTheme::default();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.write_raw("  ")?;
            fmt.success(&format!("Backup created at {}", backup_dir.display()))?;
        }
    }

    if !cli.json && tx.migration_count() > 0 {
        let mut out = io::stdout();
        let theme = ActiveTheme::default();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.write_accent("\u{2192} ")?;
        fmt.write_raw(&format!(
            "Running {} schema migration(s)...",
            tx.migration_count()
        ))?;
        fmt.newline()?;
    }

    // Run migrations
    let result = run_migrations(&cas_root, false)?;

    // Check if migrations succeeded
    if !result.errors.is_empty() {
        // Migrations failed - rollback
        if !cli.json {
            let mut out = io::stdout();
            let theme = ActiveTheme::default();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.warning("Migration errors detected, rolling back...")?;
            for (name, error) in &result.errors {
                fmt.write_raw("  ")?;
                fmt.error(&format!("{name} - {error}"))?;
            }
        }
        tx.rollback()?;
        if !cli.json {
            let mut out = io::stdout();
            let theme = ActiveTheme::default();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.success("Rolled back to backup")?;
        }
        anyhow::bail!("Migration failed, changes rolled back");
    }

    // Apply file changes
    if tx.file_change_count() > 0 {
        if !cli.json {
            let mut out = io::stdout();
            let theme = ActiveTheme::default();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.write_accent("\u{2192} ")?;
            fmt.write_raw(&format!(
                "Applying {} file change(s)...",
                tx.file_change_count()
            ))?;
            fmt.newline()?;
        }
        if let Err(e) = tx.apply_file_changes() {
            // File changes failed - rollback
            if !cli.json {
                let mut out = io::stdout();
                let theme = ActiveTheme::default();
                let mut fmt = Formatter::stdout(&mut out, theme);
                fmt.error(&format!("File update failed: {e}"))?;
                fmt.write_accent("\u{2192} ")?;
                fmt.write_raw("Rolling back...")?;
                fmt.newline()?;
            }
            tx.rollback()?;
            if !cli.json {
                let mut out = io::stdout();
                let theme = ActiveTheme::default();
                let mut fmt = Formatter::stdout(&mut out, theme);
                fmt.success("Rolled back to backup")?;
            }
            anyhow::bail!("File update failed, changes rolled back: {e}");
        }
    }

    // Success - commit transaction
    tx.commit()?;
    let final_status = check_migrations(&cas_root)?;

    if cli.json {
        let applied_json: Vec<String> = result
            .applied_names
            .iter()
            .map(|n| format!("\"{n}\""))
            .collect();

        println!(
            "{}",
            schema_status_json(
                Some(&cas_root),
                &format!(
                    r#""schema_status":"updated","current_version":{},"migrations_applied":{},"applied":[{}],"files_updated":{}"#,
                    final_status.current_version,
                    result.applied_count,
                    applied_json.join(","),
                    tx.file_change_count()
                )
            )
        );
    } else {
        let mut out = io::stdout();
        let theme = ActiveTheme::default();
        let mut fmt = Formatter::stdout(&mut out, theme);

        for name in &result.applied_names {
            fmt.write_raw("  ")?;
            fmt.success(name)?;
        }

        fmt.newline()?;
        fmt.success(&format!(
            "Schema updated to v{} — {}",
            final_status.current_version,
            cas_root.display()
        ))?;

        if args.keep_backup {
            if let Some(backup_dir) = tx.backup_dir() {
                fmt.write_raw("  ")?;
                fmt.info(&format!("Backup kept at {}", backup_dir.display()))?;
            }
        }
    }

    Ok(())
}

/// Check if a newer version is available (binary + schema)
fn check_for_updates(
    current_version: &str,
    cli: &Cli,
    cas_root_param: Option<&Path>,
) -> anyhow::Result<()> {
    use self_update::backends::github::Update;

    // Check binary updates
    let mut builder = Update::configure();
    builder
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .current_version(current_version);
    if let Some(token) = github_auth_token() {
        builder.auth_token(&token);
    }
    let updater = builder.build()?;

    let latest = updater.get_latest_release()?;
    let latest_version = latest.version.trim_start_matches('v');
    let binary_update_available = is_newer(latest_version, current_version);

    // Check schema migrations - only if cas_root is provided (Cassy is initialized)
    let schema_status = cas_root_param.and_then(|path| check_migrations(path).ok());

    let pending_migrations = schema_status.as_ref().map(|s| s.pending.len()).unwrap_or(0);

    if cli.json {
        println!(
            r#"{{"current_version":"{}","latest_version":"{}","binary_update_available":{},"schema_version":{},"pending_migrations":{}}}"#,
            current_version,
            latest_version,
            binary_update_available,
            schema_status
                .as_ref()
                .map(|s| s.current_version)
                .unwrap_or(0),
            pending_migrations
        );
        return Ok(());
    }

    let mut out = io::stdout();
    let theme = ActiveTheme::default();
    let mut fmt = Formatter::stdout(&mut out, theme);

    fmt.subheading("Binary")?;
    fmt.write_raw("  Current version: ")?;
    fmt.write_accent(current_version)?;
    fmt.newline()?;
    fmt.write_raw("  Latest version:  ")?;
    fmt.write_accent(latest_version)?;
    fmt.newline()?;

    if binary_update_available {
        fmt.newline()?;
        let success_color = fmt.theme().palette.status_success;
        fmt.write_colored("  \u{2192} ", success_color)?;
        fmt.write_raw("A new version is available!")?;
        fmt.newline()?;
        fmt.write_raw("    Run ")?;
        fmt.write_accent("cas update")?;
        fmt.write_raw(" to update")?;
        fmt.newline()?;
    } else {
        fmt.newline()?;
        fmt.write_raw("  ")?;
        fmt.success("Binary up to date")?;
    }

    fmt.newline()?;
    fmt.subheading("Schema")?;

    if let Some(status) = schema_status {
        fmt.write_raw("  Current version: ")?;
        fmt.write_accent(&format!("v{}", status.current_version))?;
        fmt.newline()?;
        fmt.write_raw("  Latest version:  ")?;
        fmt.write_accent(&format!("v{}", status.latest_version))?;
        fmt.newline()?;

        if pending_migrations > 0 {
            let warning_color = fmt.theme().palette.status_warning;
            fmt.newline()?;
            fmt.write_colored("  \u{2192} ", warning_color)?;
            fmt.write_raw(&format!("{pending_migrations} migration(s) pending"))?;
            fmt.newline()?;
            fmt.write_raw("    Run ")?;
            fmt.write_accent("cas update --dry-run")?;
            fmt.write_raw(" to preview")?;
            fmt.newline()?;
            fmt.write_raw("    Run ")?;
            fmt.write_accent("cas update --schema-only")?;
            fmt.write_raw(" to apply")?;
            fmt.newline()?;
        } else {
            fmt.newline()?;
            fmt.write_raw("  ")?;
            fmt.success("Schema up to date")?;
        }
    } else {
        fmt.write_raw("  ")?;
        fmt.warning("Cassy not initialized in this directory")?;
    }

    Ok(())
}

/// Download and install the latest (or specified) version
fn perform_update(args: &UpdateArgs, current_version: &str, cli: &Cli) -> anyhow::Result<String> {
    use self_update::Status;
    use self_update::backends::github::Update;
    use self_update::update::ReleaseUpdate;

    let mut updater = Update::configure();
    updater
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .current_version(current_version)
        .show_download_progress(true)
        .no_confirm(args.yes);
    if let Some(token) = github_auth_token() {
        updater.auth_token(&token);
    }

    // If a specific version is requested, set it
    if let Some(ref version) = args.version {
        updater.target_version_tag(&format!("v{}", version.trim_start_matches('v')));
    }

    let updater = updater.build()?;
    // self_update resolves its install destination before the replacement.
    // Keep that stable path: after a Linux rename swap, current_exe() can
    // report the old process image as `/path/cas (deleted)`.
    let installed_binary = strip_deleted_suffix(updater.bin_install_path());

    // Check what we're updating to
    let latest = updater.get_latest_release()?;
    let target_version = args
        .version
        .as_ref()
        .map(|v| v.trim_start_matches('v').to_string())
        .unwrap_or_else(|| latest.version.trim_start_matches('v').to_string());

    if !args.yes && !cli.json {
        let mut out = io::stdout();
        let theme = ActiveTheme::default();
        let mut fmt = Formatter::stdout(&mut out, theme);

        fmt.subheading("Binary Update")?;
        fmt.write_raw("  Current version: ")?;
        fmt.write_accent(current_version)?;
        fmt.newline()?;
        fmt.write_raw("  Target version:  ")?;
        fmt.write_accent(&target_version)?;
        fmt.newline()?;

        if !is_newer(&target_version, current_version) && args.version.is_none() {
            fmt.newline()?;
            fmt.write_raw("  ")?;
            fmt.success("Already on the latest version")?;
            return Ok(current_version.to_owned());
        }

        fmt.newline()?;
        fmt.write_raw("  This will download and replace the current binary.")?;
        fmt.newline()?;
    }

    // Perform the update
    let status = updater.update()?;

    if matches!(&status, Status::Updated(_)) {
        if let Err(error) = run_post_swap_hook(&installed_binary, current_version, cli.json) {
            eprintln!(
                "Post-update hook unavailable ({error}); using in-process hub restart fallback"
            );
        }
    }

    if cli.json {
        let (updated, version) = match &status {
            Status::UpToDate(v) => (false, v.as_str()),
            Status::Updated(v) => (true, v.as_str()),
        };
        println!(
            r#"{{"binary_updated":{},"version":"{}"}}"#,
            updated,
            version.trim_start_matches('v')
        );
        return Ok(version.trim_start_matches('v').to_owned());
    }

    let mut out = io::stdout();
    let theme = ActiveTheme::default();
    let mut fmt = Formatter::stdout(&mut out, theme);

    let installed_version = match status {
        Status::UpToDate(v) => {
            fmt.newline()?;
            fmt.write_raw("  ")?;
            fmt.success(&format!(
                "Already up to date ({})",
                v.trim_start_matches('v')
            ))?;
            v.trim_start_matches('v').to_owned()
        }
        Status::Updated(v) => {
            fmt.newline()?;
            fmt.write_raw("  ")?;
            fmt.success(&format!(
                "Successfully updated to {}",
                v.trim_start_matches('v')
            ))?;
            fmt.newline()?;
            fmt.write_raw("  Run ")?;
            fmt.write_accent("cas changelog")?;
            fmt.write_raw(" to see what's new")?;
            fmt.newline()?;
            v.trim_start_matches('v').to_owned()
        }
    };

    Ok(installed_version)
}

fn execute_post_swap(args: &UpdateArgs, cli: &Cli, current_version: &str) -> anyhow::Result<()> {
    // Keep the old version available to the internal protocol and future
    // diagnostics without rendering it into the normal update receipt.
    let _previous_version = args
        .from
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("post-swap mode requires --from"))?;
    super::hub::restart_stale_hub(current_version, cli)?;
    Ok(())
}

fn build_post_swap_command(
    installed_binary: &Path,
    previous_version: &str,
    json: bool,
) -> std::process::Command {
    let mut command = std::process::Command::new(installed_binary);
    command.args(["update", "--post-swap", "--from"]);
    command.arg(previous_version);
    if json {
        command.arg("--json");
    }
    command
}

fn strip_deleted_suffix(path: std::path::PathBuf) -> std::path::PathBuf {
    let Some(path_str) = path.to_str() else {
        return path;
    };
    path_str
        .strip_suffix(" (deleted)")
        .map(std::path::PathBuf::from)
        .unwrap_or(path)
}

fn run_post_swap_hook(
    installed_binary: &Path,
    previous_version: &str,
    json: bool,
) -> anyhow::Result<()> {
    let status = build_post_swap_command(installed_binary, previous_version, json)
        .status()
        .with_context(|| {
            format!(
                "run post-update hook from installed binary {}",
                installed_binary.display()
            )
        })?;
    if !status.success() {
        anyhow::bail!(
            "post-update hook from {} exited with {status}",
            installed_binary.display()
        );
    }
    Ok(())
}

/// Try to get a GitHub auth token from `gh auth token` or GITHUB_TOKEN env var.
fn github_auth_token() -> Option<String> {
    // Try GITHUB_TOKEN env var first
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        if !token.is_empty() {
            return Some(token);
        }
    }
    // Fall back to `gh auth token`
    std::process::Command::new("gh")
        .args(["auth", "token"])
        .output()
        .ok()
        .and_then(|o| {
            if o.status.success() {
                String::from_utf8(o.stdout)
                    .ok()
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            } else {
                None
            }
        })
}

/// Compare semantic versions to check if `new` is newer than `current`
fn is_newer(new: &str, current: &str) -> bool {
    let parse = |v: &str| -> Option<(u32, u32, u32)> {
        let parts: Vec<&str> = v.trim_start_matches('v').split('.').collect();
        if parts.len() >= 3 {
            Some((
                parts[0].parse().ok()?,
                parts[1].parse().ok()?,
                parts[2].split('-').next()?.parse().ok()?,
            ))
        } else {
            None
        }
    };

    match (parse(new), parse(current)) {
        (Some((n1, n2, n3)), Some((c1, c2, c3))) => (n1, n2, n3) > (c1, c2, c3),
        _ => false,
    }
}

struct UpdateStepTracker {
    total: usize,
    current: usize,
    enabled: bool,
}

impl UpdateStepTracker {
    fn new(total: usize, enabled: bool) -> Self {
        Self {
            total,
            current: 0,
            enabled,
        }
    }

    fn run<T, F>(&mut self, label: &str, f: F) -> anyhow::Result<T>
    where
        F: FnOnce() -> anyhow::Result<T>,
    {
        let step_num = self.current + 1;
        let started_at = Instant::now();
        let mut styled = false;

        if self.enabled {
            let mut out = io::stdout();
            let theme = ActiveTheme::default();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.write_accent("\u{2192} ")?;
            fmt.write_raw(&format!("[{}/{}] ", step_num, self.total))?;
            fmt.write_bold(label)?;
            styled = fmt.is_styled();
            if !styled {
                fmt.newline()?;
            } else {
                fmt.flush()?;
            }
        }

        match f() {
            Ok(value) => {
                if self.enabled {
                    let mut out = io::stdout();
                    let theme = ActiveTheme::default();
                    let mut fmt = Formatter::stdout(&mut out, theme);
                    if styled {
                        fmt.write_raw(" ")?;
                        fmt.success(&format_elapsed(started_at.elapsed()))?;
                    } else {
                        fmt.write_raw("  ")?;
                        fmt.success(&format!(
                            "{label} ({})",
                            format_elapsed(started_at.elapsed())
                        ))?;
                    }
                }
                self.current += 1;
                Ok(value)
            }
            Err(err) => {
                if self.enabled {
                    let mut out = io::stdout();
                    let theme = ActiveTheme::default();
                    let mut fmt = Formatter::stdout(&mut out, theme);
                    if styled {
                        fmt.write_raw(" ")?;
                        fmt.error(&format_elapsed(started_at.elapsed()))?;
                    } else {
                        fmt.write_raw("  ")?;
                        fmt.error(&format!(
                            "{label} ({})",
                            format_elapsed(started_at.elapsed())
                        ))?;
                    }
                }
                Err(err)
            }
        }
    }
}

fn format_elapsed(duration: Duration) -> String {
    if duration.as_secs() >= 60 {
        let mins = duration.as_secs() / 60;
        let secs = duration.as_secs() % 60;
        format!("{mins}m {secs}s")
    } else if duration.as_millis() >= 1000 {
        format!("{:.1}s", duration.as_secs_f64())
    } else {
        format!("{}ms", duration.as_millis())
    }
}

/// Write (or refresh) the skill-sync sentinel file in `<cas_root>`.
///
/// The sentinel stores a nanosecond-resolution UNIX timestamp token that
/// changes on every successful `cas update --sync` run. `handle_session_start`
/// compares this token against a per-session marker file to decide whether to
/// emit `reloadSkills: true` (cas-f9ad).
///
/// Failures are silently ignored — the hot-reload feature simply won't fire
/// when the sentinel is unwritable.
fn write_skill_sync_sentinel(cas_root: &std::path::Path) {
    use std::time::{SystemTime, UNIX_EPOCH};

    let token = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos().to_string())
        .unwrap_or_else(|_| "0".to_string());

    let _ = std::fs::write(cas_root.join("skill_sync_sentinel"), token.as_bytes());
}

#[cfg(test)]
#[path = "update_tests/tests.rs"]
mod tests;
