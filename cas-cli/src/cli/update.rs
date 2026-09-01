//! Self-update command for Cassy CLI
//!
//! Downloads and installs the latest version from GitHub releases,
//! and runs schema migrations for the local database.

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Args;

use crate::builtins::{
    SyncResult, ensure_builtin_gitignore, mark_missing_owned_references_for_replacement,
    prune_stale_user_skills_for_harness, sync_all_builtins_for_harness,
};
use crate::cli::Cli;
use crate::cli::cloud::{CloudSyncArgs, execute_sync};
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
use crate::ui::components::Formatter;
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

    // This is also the no-download entry point for a host which already has
    // the desired binary. It deliberately does the same complete sweep that a
    // successful ordinary `cas update` performs below.
    if args.all_projects {
        return refresh_all_projects(args, cli, cas_root);
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

    steps.run("Refreshing all local Cassy projects", || {
        refresh_all_projects(args, cli, cas_root)
    })?;

    if !cli.json {
        let mut out = io::stdout();
        let theme = ActiveTheme::default();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.newline()?;
        fmt.success("Update completed")?;
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
}

struct ProjectRefreshReceipt {
    project: PathBuf,
    migration: ProjectPhase,
    search_index: ProjectPhase,
    skills: ProjectPhase,
    membership: ProjectPhase,
    cloud: ProjectPhase,
}

impl ProjectRefreshReceipt {
    fn failed(&self) -> bool {
        [&self.migration, &self.skills, &self.membership, &self.cloud]
            .into_iter()
            .any(ProjectPhase::failed)
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
) -> anyhow::Result<()> {
    let projects = discover_local_projects(current_cas_root);
    let mut receipts = Vec::with_capacity(projects.len());

    if !cli.json {
        println!(
            "Refreshing {} local Cassy project(s){}",
            projects.len(),
            if args.dry_run { " (DRY RUN)" } else { "" }
        );
    }

    for project in projects {
        let cas_root = project.join(".cas");
        if !cli.json {
            println!("\n  {}", project.display());
        }

        // Run each phase independently. A malformed database must be visible
        // in the receipt, but must not leave another project stale.
        let migration = run_project_phase("migration", args.dry_run, || {
            run_schema_migrations(args, cli, Some(&cas_root))
        });
        let search_index = repair_project_search_index(&cas_root, args.dry_run, cli);
        let skills = run_project_phase("skills", args.dry_run, || {
            sync_claude_files(cli, Some(&cas_root))
        });
        let membership = refresh_project_membership(&cas_root, args.dry_run);
        let cloud = sync_project_cloud(&cas_root, args.dry_run, cli);

        receipts.push(ProjectRefreshReceipt {
            project,
            migration,
            search_index,
            skills,
            membership,
            cloud,
        });
    }

    let user_builtins = if args.dry_run {
        ProjectPhase::Planned("user-level builtins".to_string())
    } else {
        match sync_user_builtins(cli) {
            Ok(()) => ProjectPhase::Ok("user-level builtins".to_string()),
            Err(error) => ProjectPhase::Failed(error.to_string()),
        }
    };
    print_project_refresh_summary(&receipts, &user_builtins, cli);

    if receipts.iter().any(ProjectRefreshReceipt::failed) || user_builtins.failed() {
        anyhow::bail!(
            "one or more projects were not fully refreshed; see the per-project phase summary above"
        );
    }
    Ok(())
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
                "refreshed {team_count} membership(s); {adoption:?}"
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

fn sync_project_cloud(cas_root: &Path, dry_run: bool, cli: &Cli) -> ProjectPhase {
    if !cas_root.join("cloud.json").exists() {
        return ProjectPhase::Skipped("not cloud-linked".to_string());
    }
    let config = match CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root) {
        Ok(config) => config,
        Err(error) => return ProjectPhase::Failed(format!("could not read cloud config: {error}")),
    };
    if !config.is_logged_in() {
        return ProjectPhase::Skipped("not logged in — run cas login".to_string());
    }
    if dry_run {
        return ProjectPhase::Planned("cloud sync".to_string());
    }
    match execute_sync(
        &CloudSyncArgs {
            dry_run: false,
            full: false,
            rehome: false,
        },
        cli,
        cas_root,
    ) {
        Ok(()) => ProjectPhase::Ok("cloud sync".to_string()),
        Err(error) => ProjectPhase::Failed(format!("cloud sync: {error:#}")),
    }
}

fn print_project_refresh_summary(
    receipts: &[ProjectRefreshReceipt],
    user_builtins: &ProjectPhase,
    cli: &Cli,
) {
    if cli.json {
        let projects = receipts
            .iter()
            .map(|receipt| {
                serde_json::json!({
                    "project": receipt.project,
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
            serde_json::json!({ "projects": projects, "user_builtins": user_builtins.summary() })
        );
        return;
    }

    println!("\nProject refresh summary:");
    for receipt in receipts {
        println!("  {}", receipt.project.display());
        println!("    migration:  {}", receipt.migration.summary());
        println!("    search index: {}", receipt.search_index.summary());
        println!("    skills:     {}", receipt.skills.summary());
        println!("    membership: {}", receipt.membership.summary());
        println!("    cloud sync: {}", receipt.cloud.summary());
    }
    let failed = receipts.iter().filter(|receipt| receipt.failed()).count();
    println!(
        "  Total: {} succeeded, {} failed; user builtins: {}",
        receipts.len().saturating_sub(failed),
        failed,
        user_builtins.summary()
    );
}

/// Discovery is the union of the host's known-repo registry and the legacy
/// helper's scan roots. The latter is deliberately retained for binary-only
/// machines that have never seeded `known_repos`.
fn discover_local_projects(current_cas_root: Option<&Path>) -> Vec<PathBuf> {
    let mut projects = BTreeSet::new();
    if let Ok(known) = crate::worktree::discovery::list_tracked_repos() {
        for repo in known.into_iter().filter(|repo| repo.healthy) {
            projects.insert(canonical_path(&repo.path));
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
    for root in roots {
        scan_for_projects(&root, &mut projects);
    }
    if let Some(cas_root) = current_cas_root
        && let Some(project) = cas_root.parent()
    {
        projects.insert(canonical_path(project));
    }

    // `~/.cas` is host state, not a project. The legacy helper made the same
    // distinction and migrated it separately.
    if let Some(home) = dirs::home_dir() {
        projects.remove(&canonical_path(&home));
    }
    projects.into_iter().collect()
}

fn canonical_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn scan_for_projects(root: &Path, projects: &mut BTreeSet<PathBuf>) {
    if root.join(".cas").is_dir() {
        projects.insert(canonical_path(root));
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
        if matches!(
            path.file_name().and_then(|name| name.to_str()),
            Some(
                "node_modules"
                    | "target"
                    | ".git"
                    | ".cargo"
                    | ".cache"
                    | ".npm"
                    | ".rustup"
                    | ".pnpm-store"
                    | ".venv"
                    | "venv"
                    | "dist"
                    | "build"
                    | ".next"
                    | ".turbo"
            )
        ) {
            continue;
        }
        scan_for_projects(&path, projects);
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
        sync_all_builtins_for_harness(cas_mux::SupervisorCli::Claude, &claude_dir)?;
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
            sync_all_builtins_for_harness(cas_mux::SupervisorCli::Codex, &codex_dir)?;
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

        let grok_result = sync_all_builtins_for_harness(cas_mux::SupervisorCli::Grok, &grok_dir)?;
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
                println!(r#"{{"schema_status":"not_initialized","migrations_applied":0}}"#);
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
                println!(r#"{{"schema_status":"not_initialized","migrations_applied":0}}"#);
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
                r#"{{"schema_status":"up_to_date","current_version":{},"migrations_applied":0}}"#,
                status.current_version
            );
        } else {
            let mut out = io::stdout();
            let theme = ActiveTheme::default();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.success(&format!("Schema up to date (v{})", status.current_version))?;
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
            r#"{{"schema_status":"updated","current_version":{},"migrations_applied":{},"applied":[{}],"files_updated":{}}}"#,
            final_status.current_version,
            result.applied_count,
            applied_json.join(","),
            tx.file_change_count()
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
            "Schema updated to v{}",
            final_status.current_version
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

        if self.enabled {
            let mut out = io::stdout();
            let theme = ActiveTheme::default();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.write_accent("\u{2192} ")?;
            fmt.write_raw(&format!("[{}/{}] ", step_num, self.total))?;
            fmt.write_bold(label)?;
            fmt.newline()?;
        }

        match f() {
            Ok(value) => {
                if self.enabled {
                    let mut out = io::stdout();
                    let theme = ActiveTheme::default();
                    let mut fmt = Formatter::stdout(&mut out, theme);
                    fmt.write_raw("  ")?;
                    fmt.success(&format!(
                        "{label} ({})",
                        format_elapsed(started_at.elapsed())
                    ))?;
                }
                self.current += 1;
                Ok(value)
            }
            Err(err) => {
                if self.enabled {
                    let mut out = io::stdout();
                    let theme = ActiveTheme::default();
                    let mut fmt = Formatter::stdout(&mut out, theme);
                    fmt.write_raw("  ")?;
                    fmt.error(&format!(
                        "{label} ({})",
                        format_elapsed(started_at.elapsed())
                    ))?;
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
