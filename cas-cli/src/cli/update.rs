//! Self-update command for Cassy CLI
//!
//! Downloads and installs the latest version from GitHub releases,
//! and runs schema migrations for the local database.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, IsTerminal, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, RawFd};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Context;
use clap::Args;

use crate::builtins::{
    SyncResult, ensure_builtin_gitignore, harness_installed_for_user,
    mark_missing_owned_references_for_replacement, prune_stale_user_skills_for_harness,
    sync_all_builtins_for_harness, sync_all_builtins_for_project,
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
use crate::cli::init::{generate_cas_skill, update_agents_md, update_claude_md};
use crate::cli::update::preview::{build_update_transaction, show_enhanced_dry_run};
use crate::cloud::{CloudConfig, FetchTeamsOutcome, fetch_and_cache_teams, maybe_adopt_team_scope};
use crate::hybrid_search::{LegacyRepairLimits, LegacyRepairOutcome, repair_legacy_index_bounded};
use crate::migration::{check_migrations, run_migrations};
use crate::store::{open_rule_store, open_skill_store, open_store};
use crate::sync::{SkillSyncer, Syncer};
use crate::ui::components::table::Column as TableColumn;
use crate::ui::components::{Border, Formatter, OutputMode, Renderable, Table, Verdict, Width};
use crate::ui::theme::{ActiveTheme, Icons};

mod discovery_probe;
mod bindings;
pub(crate) mod findings;
mod fixture;
mod live;
mod preview;

/// Wrap the warning's prose without dropping words or adding elision. The
/// caller has already written the first prefix; continuation lines align to it.
fn write_reference_warning_prose(
    fmt: &mut Formatter<'_>,
    text: &str,
    prefix_cells: usize,
) -> io::Result<()> {
    let limit = usize::from(fmt.width().min(80));
    let mut cells = prefix_cells;
    for word in text.split_whitespace() {
        let word_cells = ratatui::text::Line::from(word).width();
        if cells > prefix_cells && cells + 1 + word_cells > limit {
            fmt.newline()?;
            fmt.write_raw(&" ".repeat(prefix_cells))?;
            cells = prefix_cells;
        }
        if cells > prefix_cells {
            fmt.write_raw(" ")?;
            cells += 1;
        }
        fmt.write_raw(word)?;
        cells += word_cells;
    }
    fmt.newline()
}

fn report_modified_builtin_references(
    result: &SyncResult,
    location: &str,
    refresh_command: &str,
    theme: &ActiveTheme,
) -> std::io::Result<()> {
    if !result.has_modified_references() {
        return Ok(());
    }

    let mut out = io::stdout();
    let mut fmt = Formatter::stdout(&mut out, theme.clone());
    let prefix_cells =
        2 + ratatui::text::Line::from(Verdict::Warning.label(fmt.glyphs())).width() + 1;
    fmt.write_raw("  ")?;
    fmt.mark(Verdict::Warning)?;
    fmt.write_raw(" ")?;
    write_reference_warning_prose(
        &mut fmt,
        &format!(
            "{} locally modified builtin reference file(s) in {location} were preserved \
             (content matches no version Cassy has shipped):",
            result.modified_reference_files.len()
        ),
        prefix_cells,
    )?;
    for file in &result.modified_reference_files {
        fmt.write_raw(&format!("    ! {file}"))?;
        fmt.newline()?;
    }
    fmt.write_raw("    ")?;
    write_reference_warning_prose(
        &mut fmt,
        "Review each file; to accept the Cassy version, delete it and rerun",
        4,
    )?;
    // Keep the command together on its own copyable line.
    fmt.write_raw(&format!("    `{refresh_command}`."))?;
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
        for file in &result.pruned_files {
            fmt.write_raw(&format!("    - {location}/{file} (no longer shipped; removed)"))?;
            fmt.newline()?;
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

    report_modified_builtin_references(result, location, "cas update --sync", theme)
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

    /// File where a post-swap refresh writes its machine-readable receipt.
    #[arg(long = "refresh-receipt", hide = true, requires = "post_swap")]
    pub refresh_receipt: Option<PathBuf>,

    /// Replay a recorded update fixture through the human renderer, for
    /// terminal review captures. Reads no store and changes nothing.
    #[arg(long = "render-fixture", hide = true, value_name = "JSON")]
    pub render_fixture: Option<PathBuf>,
}

/// cas-49c0: which `cas update` mode a run asked for, for the worker scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UpdateMode {
    Sync,
    AllProjects,
    Register,
    User,
    PostSwap,
    SchemaOnly,
    Check,
    Full,
}

impl UpdateMode {
    fn of(args: &UpdateArgs) -> Self {
        if args.post_swap {
            Self::PostSwap
        } else if args.register.is_some() {
            Self::Register
        } else if args.all_projects {
            Self::AllProjects
        } else if args.user {
            Self::User
        } else if args.sync {
            Self::Sync
        } else if args.schema_only || args.dry_run {
            Self::SchemaOnly
        } else if args.check {
            Self::Check
        } else {
            Self::Full
        }
    }
}

/// cas-49c0: what a `cas update` run may touch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkerUpdatePlan {
    /// Not a factory worker: the run behaves as before.
    Unrestricted,
    /// A worker's `--sync`: refresh only the invoking worktree's own root.
    SyncWorktree(PathBuf),
    /// A worker asked for a host-wide refresh; refused with this guidance.
    Refuse(String),
}

/// cas-49c0: a factory worker (`CAS_AGENT_ROLE=worker`) may refresh only its
/// own worktree. Its `--sync` writes the harness files under `clone_path`,
/// never the main checkout that `cas_root.parent()` names, and the modes that
/// walk the host's local project registry or install a binary are refused.
pub(crate) fn worker_update_plan(
    role: Option<&str>,
    clone_path: Option<&Path>,
    mode: UpdateMode,
) -> WorkerUpdatePlan {
    let is_worker = role.is_some_and(|role| role.trim().eq_ignore_ascii_case("worker"));
    if !is_worker {
        return WorkerUpdatePlan::Unrestricted;
    }
    let worktree = clone_path.filter(|path| !path.as_os_str().is_empty());
    match (mode, worktree) {
        (UpdateMode::Check | UpdateMode::PostSwap, _) => WorkerUpdatePlan::Unrestricted,
        (UpdateMode::Sync, Some(worktree)) => WorkerUpdatePlan::SyncWorktree(worktree.to_path_buf()),
        (UpdateMode::Sync, None) => WorkerUpdatePlan::Refuse(
            "cas update --sync refused: this factory worker has no bound worktree (CAS_CLONE_PATH), \
             so the refresh cannot be confined to it. Ask the supervisor to refresh the project."
                .to_string(),
        ),
        (_, worktree) => WorkerUpdatePlan::Refuse(format!(
            "cas update refused for a factory worker: this mode refreshes the host's local \
             projects or installs a binary, which belongs to the supervisor. Run \
             `cas update --sync` to refresh only your worktree ({}).",
            worktree
                .map(|path| path.display().to_string())
                .unwrap_or_else(|| "unbound".to_string())
        )),
    }
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

    // A review replay touches no store, binary or hub, so it runs anywhere.
    if let Some(fixture) = &args.render_fixture {
        return fixture::render(fixture, args, cli);
    }

    // A sync preview must stop before both filesystem writers and Git repair.
    if args.sync && args.dry_run {
        if cli.json {
            println!(
                "{}",
                serde_json::json!({"managed_files": {"outcome":"planned"}})
            );
        } else {
            println!("Managed files: planned; no changes written");
        }
        return Ok(());
    }

    // cas-49c0: a factory worker refreshes only its own worktree. Its
    // `--sync` used to rewrite the supervisor's main checkout, and the
    // host-wide modes walk every registered project.
    let worker_scope = {
        let role = std::env::var("CAS_AGENT_ROLE").ok();
        let clone_path = std::env::var_os("CAS_CLONE_PATH").map(PathBuf::from);
        worker_update_plan(role.as_deref(), clone_path.as_deref(), UpdateMode::of(args))
    };
    match worker_scope {
        WorkerUpdatePlan::Unrestricted => {}
        WorkerUpdatePlan::Refuse(message) => anyhow::bail!(message),
        WorkerUpdatePlan::SyncWorktree(worktree) => {
            return sync_claude_files_into(cli, cas_root, Some(&worktree))
                .and_then(check_managed_commit);
        }
    }

    if let Some(path) = &args.register {
        return register_project(path, cli);
    }

    // This is also the no-download entry point for a host which already has
    // the desired binary. It deliberately does the same complete sweep that a
    // successful ordinary `cas update` performs below.
    if args.all_projects {
        let mut steps = UpdateStepTracker::new(1, !cli.json);
        run_refresh_phase(&mut steps, cli, true, || {
            refresh_all_projects(args, cli, cas_root, None, None, None)
        })?;
        return Ok(());
    }

    // Handle user-level builtin distribution (~/.claude, ~/.codex)
    if args.user {
        if args.dry_run {
            if cli.json {
                println!("{}", serde_json::json!({"user_level": "planned"}));
            } else {
                println!("User-level builtin sync planned (dry run)");
            }
            return Ok(());
        }
        let mut steps = UpdateStepTracker::new(1, !cli.json);
        return steps.run("Distributing built-ins to user-level", || {
            sync_user_builtins(cli)
        });
    }

    // Handle sync-only mode (just sync .claude/.codex files)
    if args.sync {
        return sync_claude_files(cli, cas_root).and_then(check_managed_commit);
    }

    // Handle schema-only mode
    if args.schema_only || args.dry_run {
        let mut steps = UpdateStepTracker::new(1, !cli.json);
        steps.run("Applying schema updates", || {
            run_schema_migrations(args, cli, cas_root)
        })?;
        // GH #1170: `--schema-only` that found no initialized store migrated
        // nothing. Exiting 0 told scripts the store was current.
        if args.schema_only
            && !args.dry_run
            && !cas_root.is_some_and(crate::store::detect::cas_store_present)
        {
            anyhow::bail!(
                "no initialized Cassy store{}; nothing was migrated. Run `cas init` first.",
                cas_root
                    .map(|root| format!(" at {}", root.display()))
                    .unwrap_or_default()
            );
        }
        return Ok(());
    }

    // Handle check mode (includes schema status)
    if args.check {
        return check_for_updates(current_version, cli, cas_root);
    }

    // Full update: binary + every local project's migration/sync/cloud state.
    run_full_update(
        cli,
        !update_interactivity(args, cli, io::stdin().is_terminal()).no_confirm,
        || perform_update(args, current_version, cli),
        |pending| finish_post_swap(pending, current_version),
        |version| super::hub::restart_stale_hub(version, cli),
        |error, hub| refresh_all_projects(args, cli, cas_root, error, Some(hub), None),
    )
}

/// Called with the parsed command before Sentry/logging read configuration.
/// No writes occur here: the native sweep owns both repair and its receipt.
pub fn startup_config_notices(cli: &Cli) -> impl Drop + use<> {
    struct Scope {
        _notices: Option<crate::config::RetiredNoticeScope>,
    }
    impl Drop for Scope {
        fn drop(&mut self) {}
    }
    let native = matches!(&cli.command, Some(crate::cli::Commands::Update(args))
        if args.post_swap || (args.render_fixture.is_none() && !(args.sync && args.dry_run)
            && matches!(UpdateMode::of(args), UpdateMode::Full | UpdateMode::AllProjects)));
    Scope {
        _notices: native.then(|| {
            crate::config::RetiredNoticeScope::begin(replay_retired_config_notices, cli.verbose)
        }),
    }
}

fn replay_retired_config_notices(notices: Vec<crate::config::RetiredConfigNotice>, verbose: bool) {
    let findings = notices
        .iter()
        .map(|notice| findings::Finding {
            verdict: Verdict::Warning,
            who: vec![notice.path.display().to_string()],
            phase: "config",
            // This is an unresolved read, not a maintenance inventory.
            // Only the retirement receipt can count all supported entries.
            cause: "retired config settings remain ignored".into(),
            remedy: Some("check the config.toml above; rerun `cas update`".into()),
        })
        .collect();
    let sweep = findings::SweepFindings {
        findings,
        healthy: Vec::new(),
        unlinked: 0,
        user_level_current: false,
        project_count: 0,
        config: findings::ConfigCleanup::default(),
    };
    let mut stderr = io::stderr();
    let mut fmt = Formatter::stdout(&mut stderr, ActiveTheme::default());
    if let Ok(shortened) = findings::print(&mut fmt, &sweep) {
        if verbose {
            for notice in notices {
                let _ = fmt.write_raw(&format!(
                    "  config: {}\n  retired: {}\n",
                    notice.path.display(),
                    notice.keys.into_iter().collect::<Vec<_>>().join(", ")
                ));
            }
        } else if shortened {
            let _ = fmt.receipt("cas update --verbose shows full config paths");
        }
    }
}

/// The full update after its seams: the binary phase, the post-swap child,
/// the hub and the project refresh. `--render-fixture` replays recorded data
/// through the same path, so a review capture shows what an operator sees.
fn run_full_update(
    cli: &Cli,
    interactive: bool,
    binary: impl FnOnce() -> anyhow::Result<BinaryUpdateOutcome>,
    post_swap: impl FnOnce(&PendingPostSwap) -> anyhow::Result<()>,
    hub: impl Fn(&str) -> anyhow::Result<super::hub::HubRestartOutcome>,
    refresh: impl FnOnce(Option<&str>, &super::hub::HubRestartOutcome) -> anyhow::Result<RefreshReport>,
) -> anyhow::Result<()> {
    let mut steps = UpdateStepTracker::new(2, !cli.json);
    // A confirmation prompt needs the terminal, so only a run that cannot
    // prompt holds the phase's output behind its line.
    let (outcome, hub_restart) = steps.phase(
        "Cassy binary",
        "checking for a newer release",
        !interactive,
        || {
            let outcome = binary()?;
            // After a swap the hub follows the installed binary's refresh,
            // as it always has (cas-91ba). A phase that could not hold its
            // output (it may prompt) still holds the hub's, which its
            // findings already say (cas-70a8).
            let (hub_restart, hub_output) = match outcome.pending_post_swap {
                Some(_) => (None, String::new()),
                None => {
                    let (restart, output) =
                        capture_phase(interactive && !cli.json, || hub(&outcome.version));
                    (Some(restart?), output)
                }
            };
            Ok((outcome, hub_restart, hub_output))
        },
        |result| match result {
            Ok((outcome, _, _)) if outcome.updated => live::PhaseResult {
                verdict: Verdict::Ok,
                text: format!("updated to {}", outcome.version),
            },
            Ok((outcome, _, _)) => live::PhaseResult {
                verdict: Verdict::Ok,
                text: format!("{} is current", outcome.version),
            },
            Err(error) => match error.downcast_ref::<UpdateCancelled>() {
                Some(UpdateCancelled(current)) => live::PhaseResult {
                    verdict: Verdict::Warning,
                    text: format!("cancelled · {current} unchanged"),
                },
                None => live::PhaseResult {
                    verdict: Verdict::Error,
                    text: "update failed".to_owned(),
                },
            },
        },
        |result, output| match result {
            Ok((outcome, hub_restart, hub_output)) => {
                let mut kept = outcome
                    .notes
                    .iter()
                    .map(|note| format!("{note}\n"))
                    .collect::<String>();
                kept.push_str(&binary_phase_remainder(
                    &format!("{output}{hub_output}"),
                    &outcome.version,
                    hub_restart.as_ref(),
                ));
                kept
            }
            Err(_) => output.to_owned(),
        },
    )
    .map(|(outcome, hub_restart, _)| (outcome, hub_restart))?;

    // cas-91ba: after a swap the installed binary runs the remaining phases
    // itself, on this terminal, as phase 2 of this run.
    if let Some(pending) = &outcome.pending_post_swap {
        post_swap(pending)?;
        let (restart, output) = capture_phase(!cli.json, || hub(&outcome.version));
        let restart = restart?;
        print_indented(&binary_phase_remainder(&output, &outcome.version, Some(&restart)))?;
        return Ok(());
    }
    let hub_restart = hub_restart.unwrap_or_default();

    // cas-91ba: after a swap the installed binary has already run the
    // post-install phases. Re-running them here would replay the pre-update
    // image's behaviour and overwrite an accurate receipt with a stale one.
    if let Some(refresh) = outcome.post_install_refresh {
        if cli.json {
            println!(
                "{}",
                combined_update_receipt(
                    &outcome.version,
                    outcome.updated,
                    Some(&refresh),
                    hub_restart.transport_error.as_deref(),
                    Some(&hub_restart),
                )
            );
        }
        return Ok(());
    }

    let hub_proof = hub_restart.clone();
    let transport_error = hub_restart.transport_error.clone();
    let refreshed = refresh_after_hub_restart(hub_restart, |error| {
        run_refresh_phase(&mut steps, cli, false, || refresh(error, &hub_proof))
    });
    // cas-be05: one document. The binary's receipt carries the refresh it
    // led to, as the post-swap path above already does (cas-91ba).
    if cli.json {
        let refresh = match &refreshed {
            Ok((report, _)) => report.json_receipt.as_ref(),
            Err(error) => error
                .downcast_ref::<RefreshFailed>()
                .and_then(|failed| failed.0.json_receipt.as_ref()),
        };
        println!(
            "{}",
            combined_update_receipt(
                &outcome.version,
                outcome.updated,
                refresh,
                transport_error.as_deref(),
                Some(&hub_proof),
            )
        );
    }
    if let Err(error) = refreshed {
        // Name the binary's state beside the refresh failure, so a current
        // binary never reads as a failed update.
        return Err(match error.downcast_ref::<RefreshFailed>() {
            Some(failed) => anyhow::anyhow!(binary_exit_reason(failed, &outcome.version, outcome.updated)),
            None => error,
        });
    }

    if hub_proof.action == "failed" {
        anyhow::bail!(
            "hub recovery failed after one retry: {}; {}",
            hub_proof.failure.as_deref().unwrap_or("unknown verification error"),
            hub_proof.remedy.as_deref().unwrap_or("Run `cas hub restart --force`."),
        );
    }

    Ok(())
}

/// What the binary phase printed that its result line and the findings do
/// not already say: the version verdict and the hub's verification and
/// Serve warning are drawn from data, so their printed copies are dropped.
fn binary_phase_remainder(
    output: &str,
    version: &str,
    hub: Option<&super::hub::HubRestartOutcome>,
) -> String {
    let said = |line: &str| {
        let line = line.trim();
        let bare = line
            .trim_start_matches("[OK]")
            .trim_start_matches(Icons::CHECK)
            .trim_start();
        bare == format!("Already up to date ({version})")
            || bare == format!("Successfully updated to {version}")
            || line.starts_with("cas update: hub was ")
            // The "hub not running" finding already carries its remedy.
            || (hub.is_some_and(|hub| hub.prior_state == "none")
                && line.starts_with("cas update: hub not running"))
            || hub.is_some_and(|hub| {
                [hub.transport_warning.as_deref(), hub.remedy.as_deref()]
                    .into_iter()
                    .flatten()
                    .any(|text| line == format!("cas update: {text}"))
            })
    };
    output
        .lines()
        .filter(|line| !said(line))
        .map(|line| format!("{line}\n"))
        .collect()
}

/// One receipt for the whole `cas update` run (cas-91ba).
///
/// The binary phase and the refresh phase used to print two separate JSON
/// documents, and after the swap the refresh document came from the pre-update
/// image. Merging the installed binary's refresh receipt into the binary
/// receipt keeps a single document whose `refresh_binary_version` states which
/// image actually ran the phases.
fn combined_update_receipt(
    version: &str,
    updated: bool,
    refresh: Option<&serde_json::Value>,
    hub_transport_error: Option<&str>,
    hub_restart: Option<&super::hub::HubRestartOutcome>,
) -> serde_json::Value {
    let mut receipt = serde_json::json!({
        "binary_updated": updated,
        "version": version,
    });
    if let (Some(target), Some(refresh)) = (receipt.as_object_mut(), refresh.and_then(|r| r.as_object())) {
        for (key, value) in refresh {
            target.insert(key.clone(), value.clone());
        }
    }
    if let Some(error) = hub_transport_error {
        receipt["hub_transport"] = serde_json::json!({
            "status": "error",
            "message": error,
        });
    }
    if let Some(hub_restart) = hub_restart {
        receipt["hub_restart"] = hub_restart_receipt(hub_restart);
    }
    receipt
}

fn hub_restart_receipt(outcome: &super::hub::HubRestartOutcome) -> serde_json::Value {
    serde_json::json!({
        "from_version": outcome.previous_version,
        "to_version": outcome.current_version,
        "via": if outcome.service_managed { "service" } else { "detached" },
        "prior_state": outcome.prior_state,
        "action": outcome.action,
        "verified": outcome.verified,
        "loopback_verified": outcome.loopback_verified,
        "transport_verified": outcome.transport_verified,
        "transport_warning": outcome.transport_warning,
        "recovery_attempted": outcome.recovery_attempted,
        "public_url": outcome.public_url,
        "failure": outcome.failure,
        "remedy": outcome.remedy,
    })
}

fn refresh_after_hub_restart<T>(
    hub_restart: super::hub::HubRestartOutcome,
    refresh: impl FnOnce(Option<&str>) -> anyhow::Result<T>,
) -> anyhow::Result<(T, Option<String>)> {
    let transport_error = hub_restart.transport_error;
    let refreshed = refresh(transport_error.as_deref())?;
    Ok((refreshed, transport_error))
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
    retired_toml: Option<crate::config::ConfigRetirement>,
    migration: ProjectPhase,
    search_index: ProjectPhase,
    skills: ProjectPhase,
    membership: ProjectPhase,
    cloud: ProjectPhase,
    details: String,
    phase_details: Vec<(bool, String)>,
    /// `--json`: the documents each phase printed, nested into the receipt.
    documents: Vec<(&'static str, Vec<serde_json::Value>)>,
}

impl ProjectRefreshReceipt {
    fn warned(&self) -> bool {
        [
            &self.migration,
            &self.search_index,
            &self.skills,
            &self.membership,
            &self.cloud,
        ]
        .into_iter()
        .any(|phase| matches!(phase, ProjectPhase::Warning(_)))
    }

    fn failed(&self) -> bool {
        [&self.migration, &self.skills, &self.membership, &self.cloud]
            .into_iter()
            .any(ProjectPhase::failed)
    }
}

#[derive(Default)]
struct RefreshReport {
    project_count: usize,
    /// Failed projects plus a failed user-level store: the receipt's count.
    failed_count: usize,
    /// Failed projects only, the numerator of "N of M projects".
    failed_projects: usize,
    /// The user-level store failed; it is host state, never a project.
    user_level_failed: bool,
    /// Projects with a warned phase and none failed.
    warned_count: usize,
    /// Scan or registry roots excluded from refresh. Counted in the banner so
    /// a refresh cannot claim a clean sweep while leaving stores untouched.
    skipped_unregistered: usize,
    elapsed: Duration,
    /// What the sweep found, for the human result; `None` under `--json`.
    sweep: Option<findings::SweepFindings>,
    /// The per-project table and phase output, for `--verbose`.
    verbose_summary: String,
    /// `--json`: the refresh receipt, printed once by the caller (cas-be05).
    json_receipt: Option<serde_json::Value>,
}

impl std::fmt::Debug for RefreshReport {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RefreshReport")
            .field("project_count", &self.project_count)
            .field("failed_count", &self.failed_count)
            .field("warned_count", &self.warned_count)
            .field("skipped_unregistered", &self.skipped_unregistered)
            .finish_non_exhaustive()
    }
}

/// A scan candidate which was deliberately excluded from the refresh. Keep
/// the reason beside the path so JSON and human output cannot claim that the
/// directory was refreshed or leave the operator guessing why it was skipped.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SkippedProject {
    project: PathBuf,
    reason: String,
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
            } else if let Some(warning) = normalized.strip_prefix("warning: Violet ") {
                self.record(&format!("Violet {warning}"), project);
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
            || normalized.starts_with("warning: Violet ")
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
                // cas-91a3: a failed phase's own words (the cause, who holds
                // the store, what to do) are not always in what it printed.
                if phase.failed() && !output.contains(phase.detail()) {
                    if !output.ends_with('\n') {
                        selected.push('\n');
                    }
                    selected.push_str(&format!(
                        "{} {label}: {}\n",
                        phase.status_label(),
                        phase.detail()
                    ));
                }
            }
        }
    }
    strip_repeated_warning_lines(&selected, warnings, project, verbose, true)
}

/// The human receipt. "N failed" counts projects, as "N projects refreshed"
/// does; the user-level store is host state and is named on its own
/// (cas-06b0). `--json` keeps its own counts.
fn update_banner_text(report: &RefreshReport) -> String {
    let user = if report.user_level_failed {
        " · user-level store failed"
    } else {
        ""
    };
    let skipped = if report.skipped_unregistered > 0 {
        format!(" · {} skipped", report.skipped_unregistered)
    } else {
        String::new()
    };
    format!(
        "Cassy {} · {} projects refreshed · {} failed{user}{skipped} · {}",
        env!("CARGO_PKG_VERSION"),
        report.project_count,
        report.failed_projects,
        format_elapsed(report.elapsed)
    )
}

/// The receipt laid out for `width` cells (cas-70a8). The count grammar
/// scripts grep (`Cassy X · N projects refreshed · F failed`) always stays on
/// one line. The elapsed time is never left alone on a line: when the full
/// receipt does not fit, it goes beside the counts if there is room and is
/// otherwise dropped, because the phase line above already shows it. The
/// user-level store and skipped counts follow on their own line.
fn receipt_lines(report: &RefreshReport, width: usize) -> Vec<String> {
    let full = update_banner_text(report);
    if full.chars().count() <= width {
        return vec![full];
    }
    let core = format!(
        "Cassy {} · {} projects refreshed · {} failed",
        env!("CARGO_PKG_VERSION"),
        report.project_count,
        report.failed_projects
    );
    let mut extras = Vec::new();
    if report.user_level_failed {
        extras.push("user-level store failed".to_owned());
    }
    if report.skipped_unregistered > 0 {
        extras.push(format!("{} skipped", report.skipped_unregistered));
    }
    let elapsed = format_elapsed(report.elapsed);
    let joined = std::iter::once(core.clone()).chain(extras.iter().cloned()).collect::<Vec<_>>();
    if joined.join(" · ").chars().count() <= width {
        return vec![joined.join(" · ")];
    }
    let with_time = format!("{core} · {elapsed}");
    let first = if with_time.chars().count() <= width { with_time } else { core };
    std::iter::once(first)
        .chain((!extras.is_empty()).then(|| extras.join(" · ")))
        .collect()
}

/// One exit reason from its clauses: on one line when it fits beside the
/// error mark in 80 columns, otherwise one clause per line under the first
/// (cas-06b0), so a terminal never wraps it mid-clause.
fn exit_reason(clauses: &[String]) -> String {
    // `[ERROR] ` is the widest mark the error renderer puts before it.
    const LINE: usize = 80 - "[ERROR] ".len();
    let joined = clauses.join("; ");
    if joined.chars().count() <= LINE {
        joined
    } else {
        clauses.join("\n  ")
    }
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

/// One phase's output, split for `--json` (cas-be05): every captured line
/// that is a JSON object is a document the phase printed, nested into the
/// one final receipt; every other line is a diagnostic and goes to stderr,
/// so none is lost and none reaches stdout. Human mode keeps the text.
fn capture_phase_documents<T: UncapturedPhase>(
    cli: &Cli,
    operation: impl FnOnce() -> T,
) -> (T, String, Vec<serde_json::Value>) {
    capture_phase_documents_with(cli, PhaseCapture::new, operation)
}

/// [`capture_phase_documents`] with the capture's constructor as a seam.
///
/// Human output stays best effort: without a capture the phase still runs
/// and prints directly, as it always has. `--json` is strict: a phase whose
/// output cannot be captured is never run, because its documents would
/// reach stdout beside the one receipt. It reports itself failed instead.
fn capture_phase_documents_with<T: UncapturedPhase>(
    cli: &Cli,
    open: impl FnOnce() -> io::Result<PhaseCapture>,
    operation: impl FnOnce() -> T,
) -> (T, String, Vec<serde_json::Value>) {
    if !cli.json {
        let (value, output) = capture_phase(true, operation);
        return (value, output, Vec::new());
    }
    let mut capture = match open() {
        Ok(capture) => capture,
        Err(error) => {
            return (
                T::uncaptured(format!(
                    "not run: --json could not capture this phase's output ({error})"
                )),
                String::new(),
                Vec::new(),
            );
        }
    };
    let value = operation();
    let output = capture.finish();
    drop(capture);
    let (documents, diagnostics) = split_phase_documents(&output);
    if !diagnostics.is_empty() {
        let mut err = io::stderr();
        let _ = err.write_all(diagnostics.as_bytes());
        let _ = err.flush();
    }
    (value, String::new(), documents)
}

/// A phase result standing for a phase `--json` refused to run uncaptured.
trait UncapturedPhase {
    fn uncaptured(reason: String) -> Self;
}

impl UncapturedPhase for ProjectPhase {
    fn uncaptured(reason: String) -> Self {
        ProjectPhase::Failed(reason)
    }
}

impl UncapturedPhase for Option<ProjectPhase> {
    fn uncaptured(reason: String) -> Self {
        Some(ProjectPhase::Failed(reason))
    }
}

impl UncapturedPhase for (ProjectPhase, Vec<SyncSummary>) {
    fn uncaptured(reason: String) -> Self {
        (ProjectPhase::Failed(reason), Vec::new())
    }
}

/// The stdout/stderr capture a `--json` phase runs inside.
#[cfg(unix)]
type PhaseCapture = OutputCapture;

/// No fd capture off unix: `--json` phases report themselves not run.
#[cfg(not(unix))]
struct PhaseCapture;

#[cfg(not(unix))]
impl PhaseCapture {
    fn new() -> io::Result<Self> {
        Err(io::Error::new(io::ErrorKind::Unsupported, "stdout capture needs unix"))
    }

    fn finish(&mut self) -> String {
        String::new()
    }
}

/// JSON-object lines, in order, and everything else.
fn split_phase_documents(output: &str) -> (Vec<serde_json::Value>, String) {
    let mut documents = Vec::new();
    let mut rest = String::new();
    for line in output.lines() {
        match serde_json::from_str::<serde_json::Value>(line.trim()) {
            Ok(value @ serde_json::Value::Object(_)) => documents.push(value),
            _ if line.trim().is_empty() => {}
            _ => {
                rest.push_str(line);
                rest.push('\n');
            }
        }
    }
    (documents, rest)
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
    hub_transport_error: Option<&str>,
    hub_restart: Option<&super::hub::HubRestartOutcome>,
    first_launch_ms: Option<u64>,
) -> anyhow::Result<RefreshReport> {
    refresh_all_projects_with_user_refresh(
        args,
        cli,
        current_cas_root,
        hub_transport_error,
        hub_restart,
        first_launch_ms,
        || refresh_user_level_store(args, cli),
    )
}

fn refresh_all_projects_with_user_refresh(
    args: &UpdateArgs,
    cli: &Cli,
    current_cas_root: Option<&Path>,
    hub_transport_error: Option<&str>,
    hub_restart: Option<&super::hub::HubRestartOutcome>,
    first_launch_ms: Option<u64>,
    user_refresh: impl FnOnce() -> ProjectPhase,
) -> anyhow::Result<RefreshReport> {
    let _notices =
        crate::config::RetiredNoticeScope::begin(replay_retired_config_notices, cli.verbose);
    let started_at = Instant::now();
    let mut discovery = discover_local_projects(current_cas_root);
    for project in std::mem::take(&mut discovery.projects) {
        if discovery.git_projects.contains(&project) {
            discovery.projects.push(project);
        } else {
            discovery.skipped_unregistered.push(SkippedProject {
                project,
                reason: "not a Git project root".into(),
            });
        }
    }
    let mut receipts = Vec::with_capacity(discovery.projects.len());

    // Account registrations must converge before project proxies choose their
    // bearer. Keep this phase advisory and run it once for the whole update.
    live::set_activity("user-level store");
    let mut user_retired_toml = Vec::new();
    let mut user_roots = BTreeSet::new();
    for root in [user_level_store_root(), crate::config::global_cas_dir()]
        .into_iter()
        .flatten()
    {
        let root = root.canonicalize().unwrap_or(root);
        if user_roots.insert(root.clone()) {
            user_retired_toml.push(crate::config::retire_config(&root, args.dry_run));
        }
    }
    let (mut user_level, user_details, user_documents) =
        capture_phase_documents(cli, user_refresh);
    for maintenance in &user_retired_toml {
        let label = dirs::home_dir()
            .and_then(|home| {
                maintenance
                    .path
                    .strip_prefix(home)
                    .ok()
                    .map(|path| format!("~/{}", path.display()))
            })
            .unwrap_or_else(|| maintenance.path.display().to_string());
        user_level = with_config_retirement(user_level, maintenance, &label);
        // Failure is represented too: its cause/remedy is now in the phase.
        if let Some(root) = maintenance.path.parent() {
            crate::config::represented_retired_config(root);
        }
    }

    let names = findings::display_names(discovery.projects.iter().map(PathBuf::as_path));
    for (index, project) in discovery.projects.iter().enumerate() {
        live::set_activity(format!(
            "{} of {} · {}",
            index + 1,
            discovery.projects.len(),
            names[index]
        ));
        let project = project.clone();
        let cas_root = project.join(".cas");
        let mut details = String::new();
        let mut documents: Vec<(&'static str, Vec<serde_json::Value>)> = Vec::new();
        // Remove ignored TOML before migrations and managed-file readers load
        // config. Ownership/discovery is unchanged; never initialize a store.
        let retired_toml = crate::config::retire_config(&cas_root, args.dry_run);

        // Run each phase independently. A malformed database must be visible
        // in the receipt, but must not leave another project stale.
        let mut upkeep_notes = Vec::new();
        let (migration, output, docs) = capture_phase_documents(cli, || {
            let phase = run_project_phase("migration", args.dry_run, || {
                run_schema_migrations(args, cli, Some(&cas_root))
            });
            // The schema phase skips its closure during a dry run. Compute
            // upkeep's preview too, while retaining the schema failure gate.
            let phase = if phase.failed() {
                phase
            } else {
                match crate::config::refresh_upkeep(
                    &cas_root, env!("CARGO_PKG_VERSION"), false, args.dry_run,
                ) {
                    Ok(notes) => {
                        if args.dry_run && !cli.json && !notes.is_empty() {
                            println!("[DRY RUN] migration and docs upkeep:");
                            for note in &notes {
                                for line in findings::wrap(note, 72, false).0 {
                                    println!("  {line}");
                                }
                            }
                        }
                        upkeep_notes = notes;
                        phase
                    }
                    Err(error) => ProjectPhase::Failed(format!("migration: {error:#}")),
                }
            };
            name_store_lock_holders(phase, &cas_root)
        });
        details.push_str(&output);
        documents.push(("migration", docs));
        if !upkeep_notes.is_empty() {
            documents.push(("docs_upkeep", vec![serde_json::json!({"changes": upkeep_notes})]));
        }
        let mut phase_details = vec![(migration.is_ok(), output)];

        // cas-91a3 / cas-3af4: every later phase reads this project's store
        // with the current schema (rules.origin_project, operator_authority,
        // ...). A store the migration phase could not bring current fails
        // those reads with "no such column", which buried the one actionable
        // cause. Skip them, each naming the same reason.
        let schema_gate = (!args.dry_run)
            .then(|| store_schema_gate(&cas_root, &migration))
            .flatten();
        let (search_index, skills, membership, cloud) = if let Some(reason) = &schema_gate {
            let skipped = || ProjectPhase::Skipped(reason.clone());
            // One message, not four: a failed migration already says why
            // (and who holds the store); otherwise the first skipped phase
            // carries the reason and the rest stay quiet in the details.
            for index in 0..4 {
                phase_details.push((migration.failed() || index > 0, String::new()));
            }
            (skipped(), skipped(), skipped(), skipped())
        } else {
            let (search_index, output, docs) = capture_phase_documents(cli, || {
                repair_project_search_index(&cas_root, args.dry_run, cli)
            });
            details.push_str(&output);
            documents.push(("search_index", docs));
            phase_details.push((search_index.is_ok(), output));
            let (skills, output, docs) = capture_phase_documents(cli, || {
                refresh_project_files(cli, &cas_root, args.dry_run)
            });
            details.push_str(&output);
            documents.push(("skills", docs));
            phase_details.push((skills.is_ok(), output));
            let (membership, output, docs) = capture_phase_documents(cli, || {
                refresh_project_membership(&cas_root, args.dry_run)
            });
            details.push_str(&output);
            documents.push(("membership", docs));
            phase_details.push((membership.is_ok(), output));
            let ((cloud, _summaries), output, docs) = capture_phase_documents(cli, || {
                sync_project_cloud(&cas_root, args.dry_run, cli)
            });
            details.push_str(&output);
            documents.push(("cloud_sync", docs));
            // cas-9021: Slack channel bindings ride the cloud row; a project
            // with nothing to reconcile keeps today's receipt shape.
            let (bindings, binding_output, binding_docs) = capture_phase_documents(cli, || {
                bindings::reconcile_project_bindings(&cas_root, args.dry_run, cli.json)
            });
            let mut output = output;
            let cloud = match bindings {
                Some(bindings) => {
                    details.push_str(&binding_output);
                    output.push_str(&binding_output);
                    documents.push(("binding_reconciliation", binding_docs));
                    with_binding_reconciliation(cloud, bindings)
                }
                None => cloud,
            };
            phase_details.push((cloud.is_ok(), output));
            (search_index, skills, membership, cloud)
        };
        let skills = with_config_retirement(skills, &retired_toml, ".cas/config.toml");
        crate::config::represented_retired_config(&cas_root);

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
            retired_toml: Some(retired_toml),
            migration,
            search_index,
            skills,
            membership,
            cloud,
            details,
            phase_details,
            documents,
        });
    }

    finish_project_refresh(
        args,
        cli,
        RefreshResults {
            receipts,
            user_level,
            user_details,
            user_documents,
            user_retired_toml,
            skipped_unregistered: discovery.skipped_unregistered,
        },
        hub_transport_error,
        hub_restart,
        first_launch_ms,
        started_at,
    )
}

/// cas-9021: the cloud row carries binding reconciliation. A cloud problem
/// keeps its own cause; otherwise a binding warning becomes the row's cause.
fn with_binding_reconciliation(cloud: ProjectPhase, bindings: ProjectPhase) -> ProjectPhase {
    match (cloud, bindings) {
        (ProjectPhase::Ok(cloud), ProjectPhase::Ok(bindings)) => {
            ProjectPhase::Ok(format!("{cloud}; {bindings}"))
        }
        (
            ProjectPhase::Ok(_) | ProjectPhase::Planned(_),
            bindings @ (ProjectPhase::Warning(_) | ProjectPhase::Failed(_)),
        ) => bindings,
        (ProjectPhase::Planned(cloud), ProjectPhase::Planned(bindings)) => {
            ProjectPhase::Planned(format!("{cloud}; {bindings}"))
        }
        (cloud, _) => cloud,
    }
}

/// Keep an existing phase failure and its remedy when maintenance also fails.
/// Successful repair counts belong to the aggregate receipt, not another
/// file's failure prose. Keep unrelated failures and their remedies.
fn with_config_retirement(
    phase: ProjectPhase,
    maintenance: &crate::config::ConfigRetirement,
    label: &str,
) -> ProjectPhase {
    let detail = match maintenance.status {
        "planned" if !phase.failed() => format!(
            "{} retired TOML entries would be removed",
            maintenance.removed.len()
        ),
        "failed" => format!(
            "{label}: {}; check `{label}` and rerun `cas update`",
            maintenance.error.as_deref().unwrap_or("cleanup failed"),
        ),
        _ => return phase,
    };
    let detail = if maintenance.status == "failed" && phase.is_ok() {
        detail
    } else {
        format!("{}; {detail}", phase.detail())
    };
    if phase.failed() || maintenance.status == "failed" {
        ProjectPhase::Failed(detail)
    } else {
        match phase {
            ProjectPhase::Warning(_) => ProjectPhase::Warning(detail),
            ProjectPhase::Skipped(_) => ProjectPhase::Skipped(detail),
            ProjectPhase::Planned(_) => ProjectPhase::Planned(detail),
            _ => ProjectPhase::Ok(detail),
        }
    }
}

/// Everything one sweep learned, before any of it is printed.
struct RefreshResults {
    receipts: Vec<ProjectRefreshReceipt>,
    user_level: ProjectPhase,
    user_details: String,
    /// `--json`: what the user-level phase printed, nested into the receipt.
    user_documents: Vec<serde_json::Value>,
    user_retired_toml: Vec<crate::config::ConfigRetirement>,
    skipped_unregistered: Vec<SkippedProject>,
}

/// Report a finished sweep: the summary (or the JSON receipt), the receipt
/// file, and the exit verdict. Shared by the real sweep and the review replay.
fn finish_project_refresh(
    args: &UpdateArgs,
    cli: &Cli,
    results: RefreshResults,
    hub_transport_error: Option<&str>,
    hub_restart: Option<&super::hub::HubRestartOutcome>,
    first_launch_ms: Option<u64>,
    started_at: Instant,
) -> anyhow::Result<RefreshReport> {
    let RefreshResults {
        receipts,
        user_level,
        user_details,
        user_documents,
        user_retired_toml,
        skipped_unregistered,
    } = results;

    let mut config_cleanup = findings::ConfigCleanup::default();
    for maintenance in receipts
        .iter()
        .filter_map(|r| r.retired_toml.as_ref())
        .chain(&user_retired_toml)
    {
        config_cleanup.include(maintenance);
    }

    let failed_count = receipts.iter().filter(|receipt| receipt.failed()).count()
        + usize::from(user_level.failed());
    let mut receipt = with_first_launch_ms(
        project_refresh_receipt_json(
            &receipts,
            &user_level,
            &skipped_unregistered,
            hub_transport_error,
            hub_restart,
        ),
        first_launch_ms,
    );
    // cas-be05: what the user-level phase printed, by kind.
    if !user_documents.is_empty() {
        let (migration, builtins): (Vec<_>, Vec<_>) = user_documents
            .into_iter()
            .partition(|document| document.get("schema_status").is_some());
        receipt["user_level_store"]["phase_documents"] =
            serde_json::json!({ "migration": migration, "builtins": builtins });
    }
    if !user_retired_toml.is_empty() {
        receipt["user_level_store"]["retired_toml"] = serde_json::to_value(&user_retired_toml)?;
    }
    if receipts.iter().any(|r| r.retired_toml.is_some()) || !user_retired_toml.is_empty() {
        receipt["config_maintenance"] = serde_json::to_value(&config_cleanup)?;
    }
    if let Some(path) = &args.refresh_receipt {
        write_refresh_receipt(path, &receipt)?;
    }
    // The human result is drawn after the phase line, so it is kept as data.
    let (sweep, verbose_summary) = if cli.json {
        (None, String::new())
    } else {
        (
            Some({
                let mut sweep = findings::collect(
                    &receipts,
                    &user_level,
                    &skipped_unregistered,
                    hub_restart,
                    hub_transport_error,
                );
                sweep.config = config_cleanup;
                sweep
            }),
            // A dry run's plan is its result, so it keeps its table.
            if cli.verbose || args.dry_run {
                render_project_refresh_summary(
                    &receipts,
                    &user_level,
                    &user_details,
                    &skipped_unregistered,
                    cli.verbose,
                )
            } else {
                String::new()
            },
        )
    };
    let report = RefreshReport {
        json_receipt: cli.json.then(|| receipt.clone()),
        project_count: receipts.len(),
        failed_count,
        failed_projects: receipts.iter().filter(|receipt| receipt.failed()).count(),
        user_level_failed: user_level.failed(),
        warned_count: receipts
            .iter()
            .filter(|receipt| !receipt.failed() && receipt.warned())
            .count(),
        skipped_unregistered: skipped_unregistered.len(),
        elapsed: started_at.elapsed(),
        sweep,
        verbose_summary,
    };
    if failed_count > 0 {
        return Err(RefreshFailed(report).into());
    }
    Ok(report)
}

/// A sweep that left projects behind. It carries the whole report, so the
/// phase line and findings can still be drawn before the command fails.
#[derive(Debug)]
struct RefreshFailed(RefreshReport);

impl std::fmt::Display for RefreshFailed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let report = &self.0;
        write!(formatter, "{}", exit_reason(&self.clauses()))
    }
}

impl RefreshFailed {
    /// What was left behind, one clause each: projects, then the host store.
    fn clauses(&self) -> Vec<String> {
        let report = &self.0;
        let projects = (report.failed_projects > 0).then(|| {
            format!(
                "{} of {} projects not fully refreshed",
                report.failed_projects, report.project_count
            )
        });
        let user = report
            .user_level_failed
            .then(|| "the user-level store was not refreshed".to_owned());
        projects.into_iter().chain(user).collect()
    }
}

impl std::error::Error for RefreshFailed {}

/// A failed refresh's exit reason, ending with the binary's own state.
fn binary_exit_reason(failed: &RefreshFailed, version: &str, updated: bool) -> String {
    let mut clauses = failed.clauses();
    clauses.push(format!(
        "Cassy {version} is {}",
        if updated { "installed" } else { "current" }
    ));
    exit_reason(&clauses)
}

/// The report behind a refresh result, whether or not it failed.
fn refresh_report(result: &anyhow::Result<RefreshReport>) -> Option<&RefreshReport> {
    match result {
        Ok(report) => Some(report),
        Err(error) => error.downcast_ref::<RefreshFailed>().map(|failed| &failed.0),
    }
}

/// The phase line's result for a sweep: how many projects converged.
fn refresh_phase_result(result: &anyhow::Result<RefreshReport>) -> live::PhaseResult {
    let Some(report) = refresh_report(result) else {
        return live::PhaseResult {
            verdict: Verdict::Error,
            text: "did not finish".to_owned(),
        };
    };
    let total = report.project_count;
    let mut text = if report.failed_projects > 0 {
        format!("{} of {total} failed", report.failed_projects)
    } else if report.user_level_failed {
        "user-level store failed".to_owned()
    } else if total == 1 {
        "1 project refreshed".to_owned()
    } else {
        format!("all {total} refreshed")
    };
    if report.failed_projects > 0 && report.user_level_failed {
        text.push_str(" · user-level store failed");
    }
    if report.warned_count > 0 {
        text.push_str(&format!(
            " · {} {}",
            report.warned_count,
            if report.warned_count == 1 { "warning" } else { "warnings" }
        ));
    }
    if report.skipped_unregistered > 0 {
        text.push_str(&format!(" · {} skipped", report.skipped_unregistered));
    }
    let verdict = if report.failed_count > 0 {
        Verdict::Error
    } else if report.warned_count > 0 || report.skipped_unregistered > 0 {
        Verdict::Warning
    } else {
        Verdict::Ok
    };
    live::PhaseResult { verdict, text }
}

/// Run the sweep as one phase line, then draw what it found.
fn run_refresh_phase(
    steps: &mut UpdateStepTracker,
    cli: &Cli,
    print_json: bool,
    refresh: impl FnOnce() -> anyhow::Result<RefreshReport>,
) -> anyhow::Result<RefreshReport> {
    let result = steps.phase(
        "Projects",
        "finding projects",
        true,
        refresh,
        refresh_phase_result,
        |_, output| output.to_owned(),
    );
    if let Some(report) = refresh_report(&result) {
        if !cli.json {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
            print_sweep(&mut fmt, report, cli.verbose)?;
        } else if print_json && let Some(receipt) = &report.json_receipt {
            // cas-be05: the one document `--json` prints, failed or not.
            println!("{receipt}");
        }
    }
    result
}

/// The findings, then one muted receipt with the count grammar scripts grep.
fn print_sweep(fmt: &mut Formatter<'_>, report: &RefreshReport, verbose: bool) -> io::Result<()> {
    fmt.newline()?;
    if !report.verbose_summary.is_empty() {
        fmt.write_raw(&report.verbose_summary)?;
        fmt.newline()?;
    }
    let shortened = match &report.sweep {
        Some(sweep) => findings::print(fmt, sweep)?,
        None => false,
    };
    for line in receipt_lines(report, usize::from(fmt.width()).clamp(40, 100) - 2) {
        fmt.write_raw("  ")?;
        fmt.receipt(&line)?;
    }
    if !verbose
        && (shortened || report.sweep.as_ref().is_some_and(|sweep| !sweep.findings.is_empty()))
    {
        fmt.write_raw("  ")?;
        fmt.receipt("cas update --verbose shows every project and phase output")?;
    }
    Ok(())
}

/// Refresh the user-level store (`~/.cas`) as its own phase.
///
/// Before cas-9d5c the all-projects sweep distributed user-level builtins but
/// never ran migrations on `~/.cas`, so the host store drifted behind every
/// project on the machine while the receipt reported a clean run. It is
/// deliberately not a project: it is never counted in the project totals and
/// never appears in the project table.
fn refresh_user_level_store(args: &UpdateArgs, cli: &Cli) -> ProjectPhase {
    refresh_user_level_store_with_builtins(args, cli, || sync_user_builtins(cli))
}

fn refresh_user_level_store_with_builtins(
    args: &UpdateArgs,
    cli: &Cli,
    builtins: impl FnOnce() -> anyhow::Result<()>,
) -> ProjectPhase {
    let Some(cas_root) = user_level_store_root() else {
        return ProjectPhase::Skipped("no home directory".to_string());
    };
    if args.dry_run {
        return ProjectPhase::Planned(format!("migrate {} and sync builtins", cas_root.display()));
    }

    if !cas_root.join("cas.db").is_file() {
        return match builtins() {
            Ok(()) => ProjectPhase::Ok(format!(
                "builtins only — no store at {}",
                cas_root.display()
            )),
            Err(error) => ProjectPhase::Failed(error.to_string()),
        };
    }
    let migration = run_project_phase("user-level migration", false, || {
        run_schema_migrations(args, cli, Some(&cas_root))
    });
    if migration.failed() {
        return migration;
    }
    match builtins() {
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

/// Why a project's later refresh phases must not read its store, or `None`
/// when the store's schema is current (cas-91a3, cas-3af4).
fn store_schema_gate(cas_root: &Path, migration: &ProjectPhase) -> Option<String> {
    if migration.failed() {
        return Some("schema not current: the migration phase failed (see migr)".to_string());
    }
    if !cas_root.join("cas.db").exists() {
        return None;
    }
    match check_migrations(cas_root) {
        Ok(status) if status.pending.is_empty() => None,
        Ok(status) => Some(format!(
            "schema not current: v{} of v{}, {} migration(s) pending; rerun `cas update` in {}",
            status.current_version,
            status.latest_version,
            status.pending.len(),
            cas_root.parent().unwrap_or(cas_root).display()
        )),
        Err(error) => Some(format!("schema not readable: {error}")),
    }
}

/// A migration refused by SQLite's write lock names the processes holding the
/// store open and what to do, as the hub machine-lock message does (cas-91a3).
fn name_store_lock_holders(phase: ProjectPhase, cas_root: &Path) -> ProjectPhase {
    let ProjectPhase::Failed(detail) = phase else {
        return phase;
    };
    if !is_store_lock_error(&detail) {
        return ProjectPhase::Failed(detail);
    }
    ProjectPhase::Failed(format!(
        "{detail}; {}",
        store_lock_remedy(cas_root, &store_lock_holders(cas_root))
    ))
}

fn is_store_lock_error(detail: &str) -> bool {
    let detail = detail.to_ascii_lowercase();
    detail.contains("database is locked") || detail.contains("database table is locked")
}

fn store_lock_holders(cas_root: &Path) -> Vec<crate::hub::HubLockHolder> {
    let mut holders = Vec::new();
    for name in ["cas.db", "cas.db-wal", "cas.db-shm"] {
        let path = cas_root.join(name);
        let path = path.canonicalize().unwrap_or(path);
        holders.extend(crate::hub::processes_holding_file(&path));
    }
    holders.sort_by_key(|holder| holder.pid);
    holders.dedup_by_key(|holder| holder.pid);
    holders
}

fn store_lock_remedy(cas_root: &Path, holders: &[crate::hub::HubLockHolder]) -> String {
    let project = cas_root.parent().unwrap_or(cas_root).display();
    let named = holders
        .iter()
        .map(|holder| match holder.command.as_deref() {
            Some(command) => format!("pid {} ({command})", holder.pid),
            None => format!("pid {}", holder.pid),
        })
        .collect::<Vec<_>>();
    if named.is_empty() {
        format!(
            "another process holds its write lock; stop the Cassy sessions in {project} and rerun `cas update`"
        )
    } else {
        format!(
            "store held open by {}; stop the Cassy sessions in {project} and rerun `cas update`",
            named.join(", ")
        )
    }
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

// Deliberate staging remains inspectable; an attempted commit failure is an error.
fn check_managed_commit(report: crate::cli::project_files::Report) -> anyhow::Result<()> {
    if let Some(error) = report.commit_error {
        // Main owns the one final error. Leave room for its widest mark;
        // preserve full Git stderr and hook paths in the JSON report.
        let (lines, shortened) = findings::wrap(&error, 80 - "[ERROR] ".len(), false);
        let mut message = lines.join("\n  ");
        if shortened {
            message.push_str("\n  Full details: cas update --sync --json");
        }
        anyhow::bail!(message);
    }
    Ok(())
}

// Preserve the managed-file verdict through the compact sweep. In particular,
// a skipped writer must never become an OK skills phase with its warning hidden.
fn refresh_project_files(cli: &Cli, cas_root: &Path, dry_run: bool) -> ProjectPhase {
    if dry_run {
        return ProjectPhase::Planned("skills".into());
    }
    match sync_claude_files(cli, Some(cas_root)) {
        Err(error) => ProjectPhase::Failed(format!("skills: {error:#}")),
        Ok(report) if report.commit_error.is_some() => {
            ProjectPhase::Failed(report.commit_error.unwrap())
        }
        Ok(report) if !report.conflicts.is_empty() => ProjectPhase::Warning(format!(
            "{} managed file conflict(s); run `cas update --sync --json`",
            report.conflicts.len()
        )),
        Ok(report) if report.outcome == "staged" => {
            ProjectPhase::Warning("managed changes staged; commit pending".into())
        }
        Ok(report) => ProjectPhase::Ok(format!("skills; managed files {}", report.outcome)),
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
            background: false,
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

/// The machine-readable refresh receipt.
///
/// Split out of the printer so the shape is testable, and so the post-swap
/// child and the in-process path emit exactly the same document (cas-91ba).
fn project_refresh_receipt_json(
    receipts: &[ProjectRefreshReceipt],
    user_level: &ProjectPhase,
    skipped_unregistered: &[SkippedProject],
    hub_transport_error: Option<&str>,
    hub_restart: Option<&super::hub::HubRestartOutcome>,
) -> serde_json::Value {
    let projects = receipts
        .iter()
        .map(|receipt| {
            let mut value = serde_json::json!({
                "project": receipt.project,
                "store": receipt.project.join(".cas"),
                "registered": !receipt.unregistered,
                "migration": receipt.migration.summary(),
                "search_index": receipt.search_index.summary(),
                "skills": receipt.skills.summary(),
                "membership": receipt.membership.summary(),
                "cloud_sync": receipt.cloud.summary(),
            });
            if let Some(maintenance) = &receipt.retired_toml {
                value["retired_toml"] = serde_json::json!(maintenance);
            }
            value
        })
        .zip(receipts)
        .map(|(mut project, receipt)| {
            // cas-be05: under --json, what each phase printed, by phase.
            if receipt.documents.iter().any(|(_, documents)| !documents.is_empty()) {
                project["phase_documents"] = receipt
                    .documents
                    .iter()
                    .map(|(phase, documents)| ((*phase).to_owned(), serde_json::json!(documents)))
                    .collect::<serde_json::Map<_, _>>()
                    .into();
            }
            project
        })
        .collect::<Vec<_>>();
    let mut receipt = serde_json::json!({
        // cas-91ba: name the binary that actually performed this refresh. A
        // receipt from the pre-update image is what made an operator's first
        // `cas update` look converged when it was not.
        "refresh_binary_version": env!("CARGO_PKG_VERSION"),
        "refresh_status": if hub_restart.is_some_and(|hub| hub.action == "failed")
            || receipts.iter().any(|receipt| receipt.failed())
            || user_level.failed()
        {
            "refresh_failed"
        } else {
            "complete"
        },
        "projects": projects,
        "user_level_store": {
            "store": user_level_store_root(),
            "status": user_level.summary(),
        },
        // Retained for compatibility with readers of the pre-cas-9d5c
        // receipt, which only ever reported the builtin distribution.
        "user_builtins": user_level.summary(),
        "skipped_unregistered": skipped_unregistered
            .iter()
            .map(|skip| serde_json::json!({
                "project": skip.project,
                "reason": skip.reason,
            }))
            .collect::<Vec<_>>(),
    });
    if let Some(error) = hub_transport_error {
        receipt["hub_transport"] = serde_json::json!({
            "status": "error",
            "message": error,
        });
    }
    if let Some(hub_restart) = hub_restart {
        receipt["hub_restart"] = hub_restart_receipt(hub_restart);
    }
    receipt
}

fn write_refresh_receipt(path: &Path, receipt: &serde_json::Value) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec(receipt).context("could not encode refresh receipt")?;
    std::fs::write(path, bytes)
        .with_context(|| format!("could not write refresh receipt {}", path.display()))?;
    Ok(())
}

/// The compact (non-JSON) refresh summary: the project table, a details
/// block under each project with a warned, failed or dry-run planned phase
/// (every project with `verbose`), the user-level line and skipped projects
/// (cas-8030, cas-937a).
fn render_project_refresh_summary(
    receipts: &[ProjectRefreshReceipt],
    user_level: &ProjectPhase,
    user_details: &str,
    skipped_unregistered: &[SkippedProject],
    verbose: bool,
) -> String {
    use std::fmt::Write as _;
    let mut out = String::new();
    let mut warnings = RepeatedWarningCollector::default();
    let mut details = Vec::with_capacity(receipts.len());
    for receipt in receipts {
        let project = project_display_name(&receipt.project, verbose);
        // Collect warnings from every phase, including successful phases whose
        // output is intentionally omitted from the compact transcript.
        warnings.collect_output(&project, &receipt.details);
        let detail = render_project_phase_details(receipt, verbose, &mut warnings, &project);
        let show_detail = verbose
            || [
                &receipt.migration,
                &receipt.search_index,
                &receipt.skills,
                &receipt.membership,
                &receipt.cloud,
            ]
            .into_iter()
            // cas-8030: a skipped phase ("not cloud-linked") is already
            // explained by the table's note, so it earns no details block.
            // A dry run's planned phases do (cas-937a): their block is the
            // only place the compact view says "DRY RUN" and what would run.
            .any(|phase| {
                matches!(
                    phase,
                    ProjectPhase::Warning(_) | ProjectPhase::Failed(_) | ProjectPhase::Planned(_)
                )
            });
        details.push((project, show_detail.then_some(detail)));
    }
    if !user_details.is_empty() {
        let project = "user-level";
        let detail =
            strip_repeated_warning_lines(user_details, &mut warnings, project, verbose, false);
        if verbose && !detail.trim().is_empty() {
            details.push((project.to_owned(), Some(detail)));
        }
    }

    let table_lines = render_project_table_plain(receipts, verbose);
    let mut table_lines = table_lines.lines();
    if let Some(header) = table_lines.next() {
        let _ = writeln!(out, "{header}");
    }
    for ((_, row), (project, detail)) in receipts.iter().zip(table_lines).zip(details) {
        let _ = writeln!(out, "{row}");
        if let Some(detail) = detail
            && !detail.trim().is_empty()
        {
            let _ = writeln!(out, "  {project} details:");
            for line in detail.lines().filter(|line| !line.trim().is_empty()) {
                let _ = writeln!(out, "    {line}");
            }
        }
    }

    // The user-level store is host state, so it gets a named line of its own
    // rather than a project row it would otherwise be miscounted in.
    let _ = writeln!(
            out,
        "  [{}] user-level store: {}",
        user_level.status_label().trim_matches(['[', ']']),
        user_level.detail()
    );

    if !skipped_unregistered.is_empty() {
        let _ = writeln!(
            out,
            "  not refreshed (skipped_unregistered): {} — use `cas known-repos forget <path>` for stale registry rows; intentional projects can be registered explicitly",
            skipped_unregistered.len()
        );
        for skip in skipped_unregistered.iter().take(5) {
            let _ = writeln!(out, "    ! {} — {}", skip.project.display(), skip.reason);
        }
        if skipped_unregistered.len() > 5 {
            let _ = writeln!(out, "    … and {} more", skipped_unregistered.len() - 5);
        }
    }
    out.push_str(&warnings.render(verbose));
    out
}

fn with_first_launch_ms(
    mut receipt: serde_json::Value,
    first_launch_ms: Option<u64>,
) -> serde_json::Value {
    if let Some(first_launch_ms) = first_launch_ms {
        receipt["binary_first_launch_ms"] = serde_json::json!(first_launch_ms);
    }
    receipt
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
    /// Git ownership observed under the same per-root deadline.
    git_projects: BTreeSet<PathBuf>,
    /// The subset of `projects` that the filesystem scan found but the host
    /// registry does not know about. These are refreshed and then registered,
    /// so discovery converges without the operator doing anything.
    unregistered: BTreeSet<PathBuf>,
    /// Scan-discovered or registered directories which are not safe project
    /// roots. There is nothing to migrate or sync, so they are reported rather
    /// than refreshed — previously they were silently absent from the receipt
    /// entirely.
    skipped_unregistered: Vec<SkippedProject>,
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
    use crate::store::KnownRepoStore;
    use crate::store::known_repos::{open_host_known_repo_store, registry_skip_for_observed_roots};

    // Do not call list_tracked_repos here: its synchronous health checks touch
    // each root before the bounded observer can protect the refresh.
    let known = open_host_known_repo_store()
        .and_then(|store| store.list().map_err(Into::into))
        .unwrap_or_default();
    let mut inspector = discovery_probe::Inspector::default();
    let mut observations = Vec::new();
    let mut known_artifacts = Vec::new();
    let mut skipped_by_path = BTreeMap::new();
    for repo in known {
        match inspector.observe(&repo.path) {
            Some(root) if root.ready => {
                known_artifacts.push((root.root.clone(), root.artifacts.clone()));
                observations.push(root);
            }
            Some(_) => {}
            None => {
                // Even an unreadable registered parent still owns its literal
                // artifact subtree; do not turn its copies into live projects.
                known_artifacts.push((repo.path.clone(), vec![repo.path.join(".cas/artifacts")]));
                skipped_by_path.insert(repo.path, discovery_probe::TIMEOUT.to_string());
            }
        }
    }
    let classify = |root: &discovery_probe::Observation| {
        registry_skip_for_observed_roots(
            root.skip.clone(),
            std::slice::from_ref(&root.root),
            &known_artifacts,
        )
    };
    let mut registered = BTreeSet::new();
    for root in observations {
        if let Some(skip) = classify(&root) {
            skipped_by_path.insert(root.root, skip.reason());
        } else {
            registered.insert(root.root);
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
        scan_observed_projects(
            &root,
            MAX_SCAN_DEPTH,
            &mut scanned,
            &mut inspector,
            &mut skipped_by_path,
        );
    }
    let home = dirs::home_dir().map(|home| {
        inspector
            .observe(&home)
            .map(|root| root.root)
            .unwrap_or(home)
    });
    let mut projects = registered.clone();
    let mut unregistered = BTreeSet::new();
    for candidate in scanned {
        if home.as_ref() == Some(&candidate) || registered.contains(&candidate) {
            continue;
        }
        let Some(root) = inspector.observe(&candidate) else {
            continue;
        };
        if root.has_store {
            if let Some(skip) = classify(&root) {
                skipped_by_path.insert(candidate, skip.reason());
            } else {
                projects.insert(candidate.clone());
                unregistered.insert(candidate);
            }
        } else {
            skipped_by_path.insert(candidate, "no .cas/cas.db store".to_string());
        }
    }
    // Explicit current roots have exactly the same ownership checks as the
    // registry and scan, and share an already timed-out observation.
    if let Some(project) = current_cas_root.and_then(Path::parent) {
        match inspector.observe(project) {
            Some(root) if home.as_ref() != Some(&root.root) => {
                if !root.has_store {
                    skipped_by_path.insert(root.root, "no .cas/cas.db store".into());
                } else if let Some(skip) = classify(&root) {
                    skipped_by_path.insert(root.root, skip.reason());
                } else {
                    projects.insert(root.root);
                }
            }
            Some(_) => {}
            None => {
                skipped_by_path.insert(project.to_path_buf(), discovery_probe::TIMEOUT.into());
            }
        }
    }
    if let Some(home) = home {
        projects.remove(&home);
        unregistered.remove(&home);
    }
    let mut seen_inodes = std::collections::HashSet::new();
    let mut preferred = projects.into_iter().collect::<Vec<_>>();
    preferred.sort_by_key(|path| (!registered.contains(path), path.clone()));
    let mut projects = BTreeSet::new();
    let mut git_projects = BTreeSet::new();
    for project in preferred {
        let Some(root) = inspector.observe(&project) else {
            continue;
        };
        if root.inode.is_some_and(|inode| !seen_inodes.insert(inode)) {
            unregistered.remove(&project);
            skipped_by_path.insert(project, "duplicate project directory".to_string());
        } else {
            if root.git_root {
                git_projects.insert(project.clone());
            }
            projects.insert(project);
        }
    }
    ProjectDiscovery {
        projects: projects.into_iter().collect(),
        git_projects,
        unregistered,
        skipped_unregistered: skipped_by_path
            .into_iter()
            .map(|(project, reason)| SkippedProject { project, reason })
            .collect(),
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
#[cfg(test)]
fn scan_for_projects(root: &Path, depth: usize, projects: &mut BTreeSet<PathBuf>) {
    scan_observed_projects(
        root,
        depth,
        projects,
        &mut discovery_probe::Inspector::default(),
        &mut BTreeMap::new(),
    );
}

fn scan_observed_projects(
    root: &Path,
    depth: usize,
    projects: &mut BTreeSet<PathBuf>,
    inspector: &mut discovery_probe::Inspector,
    skipped: &mut BTreeMap<PathBuf, String>,
) {
    let Some(observation) = inspector.observe(root) else {
        skipped.insert(root.to_path_buf(), discovery_probe::TIMEOUT.into());
        return;
    };
    if observation.project_root {
        projects.insert(observation.root);
    }
    if depth == 0 {
        return;
    }
    for child in observation.children {
        scan_observed_projects(&child, depth - 1, projects, inspector, skipped);
    }
}

/// Sync rules, skills, and configuration to .claude/.codex directories
fn sync_claude_files(
    cli: &Cli,
    cas_root_param: Option<&Path>,
) -> anyhow::Result<crate::cli::project_files::Report> {
    sync_claude_files_into(cli, cas_root_param, None)
}

/// [`sync_claude_files`] writing the harness files under `project_root`
/// instead of `cas_root.parent()` (cas-49c0: a worker's own worktree).
fn sync_claude_files_into(
    cli: &Cli,
    cas_root_param: Option<&Path>,
    project_root: Option<&Path>,
) -> anyhow::Result<crate::cli::project_files::Report> {
    // cas_root is optional - if not provided and Cassy is not initialized, nothing to sync
    let cas_root = match cas_root_param {
        Some(path) => path.to_path_buf(),
        None => {
            // Not initialized, nothing to sync
            return Ok(crate::cli::project_files::Report {
                outcome: "skipped",
                ..Default::default()
            });
        }
    };

    let _refresh_host_integration = project_root.is_none();
    let project_root = project_root.unwrap_or_else(|| cas_root.parent().unwrap_or(&cas_root));
    let claude_dir = project_root.join(".claude");
    let codex_dir = project_root.join(".codex");
    let codex_enabled = codex_dir.exists();
    let grok_dir = project_root.join(".grok");
    // cas-6b97 (audit M39/M40): an installed Grok or OpenCode ranks the
    // project's own `.grok/skills` / `.opencode/skills` above `.claude/skills`,
    // so write their projection whenever they are installed. Without it they
    // load the Claude-spelled copies, whose tool names do not resolve for them.
    let grok_enabled =
        grok_dir.exists() || harness_installed_for_user(cas_mux::SupervisorCli::Grok);
    let opencode_dir = project_root.join(".opencode");
    let opencode_enabled =
        opencode_dir.exists() || harness_installed_for_user(cas_mux::SupervisorCli::OpenCode);

    let theme = ActiveTheme::default();

    // Builtin files are generated into consumer projects and should not be
    // committed there. The authoring checkout is exempted inside the helper
    // because its rendered files are intentional tracked fixtures.
    let builtin_harnesses = crate::cli::project_files::harnesses(project_root);
    let reconciliation =
        match crate::cli::project_files::Reconcile::begin(project_root, &builtin_harnesses)? {
            Some(plan) if plan.blocked() => {
                report_managed_files(cli, &plan.report)?;
                return Ok(plan.report);
            }
            plan => plan,
        };
    // A worktree-scoped sync (cas-49c0) leaves host-level installs alone.
    #[cfg(feature = "mcp-proxy")]
    if _refresh_host_integration {
        crate::cli::integrate::violet_retirement::retire_installed_hub(Some(
            &cas_root.join("proxy.toml"),
        ))?;
        crate::cli::integrate::violet_retirement::repair_proxy_credentials_on_update(Some(
            &cas_root.join("proxy.toml"),
        ));
    }
    let mut builtin_gitignore = ensure_builtin_gitignore(project_root, &builtin_harnesses)?;

    if !cli.json {
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme.clone());
        fmt.subheading("Syncing .claude files")?;
        if builtin_gitignore.updated {
            fmt.write_raw("  ")?;
            fmt.success("Updated .gitignore with Cassy-managed builtin paths")?;
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

    // 3b. AGENTS.md directive: the harness-neutral block CLAUDE.md imports
    // and Codex/Grok read directly (audit D3, M66).
    match update_agents_md(project_root) {
        Ok(true) => {
            config_updated.push("AGENTS.md");
            if !cli.json {
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme.clone());
                fmt.write_raw("  ")?;
                fmt.success("Updated AGENTS.md")?;
            }
        }
        Ok(false) => {}
        Err(e) => {
            if !cli.json {
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme.clone());
                fmt.write_raw("  ")?;
                fmt.warning(&format!("Could not update AGENTS.md: {e}"))?;
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
    let mut rules = rule_store.list()?;
    // cas-caae (skills audit M27): keep rules naming another registered
    // project out of this project's Claude Code rules.
    crate::store::foreign_project_guard::ForeignProjectGuard::for_project_root(project_root)
        .retain_syncable_rules(&mut rules);
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
    if opencode_enabled {
        mark_missing_owned_references_for_replacement(
            cas_mux::SupervisorCli::OpenCode,
            &opencode_dir,
        )?;
    }

    // Sync database skills (this may remove stale dirs)
    let skill_store = open_skill_store(&cas_root)?;
    let skills = skill_store.list(None)?;
    let skill_syncer = SkillSyncer::with_defaults(project_root);
    // Preserve project-authored cas-* siblings; builtin pruning already
    // belongs to its own writer. Explicit disabled database
    // skills are still handled by sync_skill inside this operation.
    let retained = skill_syncer.list_synced()?.into_iter().collect();
    let skill_report = skill_syncer.sync_all_preserving(&skills, &retained)?;

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

    let opencode_builtins_updated = if opencode_enabled {
        if !cli.json {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme.clone());
            fmt.subheading("Syncing .opencode files")?;
        }
        let opencode_result =
            sync_all_builtins_for_project(cas_mux::SupervisorCli::OpenCode, project_root)?;
        if !cli.json {
            report_builtin_sync(&opencode_result, ".opencode", &theme)?;
        }
        opencode_result.total_updated()
    } else {
        0
    };

    let managed_files = if let Some(plan) = reconciliation {
        let report = plan.finish(&builtin_harnesses)?;
        builtin_gitignore.tracked_paths.clear();
        report
    } else {
        crate::cli::project_files::Report {
            outcome: "skipped",
            ..Default::default()
        }
    };
    if !cli.json {
        report_managed_files(cli, &managed_files)?;
    }

    if cli.json {
        let config_json: Vec<String> = config_updated.iter().map(|s| format!("\"{s}\"")).collect();
        let codex_config_json: Vec<String> = codex_config_updated
            .iter()
            .map(|s| format!("\"{s}\""))
            .collect();
        println!(
            r#"{{"config_updated":[{}],"builtins_updated":{},"builtin_reference_conflicts":{},"codex_config_updated":[{}],"codex_builtins_updated":{},"codex_builtin_reference_conflicts":{},"grok_builtins_updated":{},"grok_builtin_reference_conflicts":{},"opencode_builtins_updated":{},"rules_synced":{},"rules_removed":{},"skills_synced":{},"skills_removed":{},"factory_tooling":"{}","builtin_gitignore_updated":{},"builtin_gitignore_tracked":{},"managed_files":{}}}"#,
            config_json.join(","),
            builtin_result.total_updated(),
            builtin_result.modified_reference_files.len(),
            codex_config_json.join(","),
            codex_builtins_updated,
            codex_modified_references,
            grok_builtins_updated,
            grok_modified_references,
            opencode_builtins_updated,
            rule_report.synced,
            rule_report.removed,
            skill_report.synced,
            skill_report.removed,
            factory_tooling_result,
            builtin_gitignore.updated,
            serde_json::to_string(&builtin_gitignore.tracked_paths)?,
            serde_json::to_string(&managed_files)?
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

    Ok(managed_files)
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
    sync_user_builtins_with_violet(cli, || {
        #[cfg(feature = "mcp-proxy")]
        crate::cli::integrate::violet_retirement::migrate_installed_hub_on_update()?;
        Ok(())
    })
}

fn sync_user_builtins_with_violet(
    cli: &Cli,
    violet: impl FnOnce() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    if let Err(error) = violet() {
        eprintln!("warning: Violet update advisory: {error}; continuing builtin sync");
    }
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
        // Prune legacy non-managed cas-* orphans (e.g. a retired, unmarked skill
        // dir) the project-level sync already drops but the user-level path
        // historically never did (cas-e0d1). The old unmarked
        // cas-playwright-debug copy is now overwritten by the managed builtin
        // of the same name (cas-5e54) instead of being pruned.
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
            for file in &r.pruned_files {
                fmt.write_raw(&format!("    - {file} (no longer shipped; removed)"))?;
                fmt.newline()?;
            }
            drop(fmt);
            report_modified_builtin_references(&r, "~/.claude", "cas update --user", &theme)?;
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
            for file in &r.pruned_files {
                fmt.write_raw(&format!("    - {file} (no longer shipped; removed)"))?;
                fmt.newline()?;
            }
            drop(fmt);
            report_modified_builtin_references(&r, "~/.codex", "cas update --user", &theme)?;
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
            for file in &r.pruned_files {
                fmt.write_raw(&format!("    - {file} (no longer shipped; removed)"))?;
                fmt.newline()?;
            }
            drop(fmt);
            report_modified_builtin_references(&r, "~/.grok", "cas update --user", &theme)?;
        }
        Some(r)
    } else {
        // No nag for absent ~/.grok — Grok is opt-in and most users won't
        // have it. Silent skip.
        None
    };

    // Named accounts and linked shared bases can outlive the retired default
    // dirs. Refresh only finish here; their other configuration stays local.
    for (provider, harness) in [
        (
            crate::accounts::Provider::Claude,
            cas_mux::SupervisorCli::Claude,
        ),
        (
            crate::accounts::Provider::Codex,
            cas_mux::SupervisorCli::Codex,
        ),
    ] {
        let default_dir = home.join(format!(".{}", provider.name()));
        let mut accounts: Vec<_> = crate::accounts::discover(provider, &home)?
            .into_iter()
            .map(|account| account.path)
            .filter(|path| *path != default_dir)
            .collect();
        let base = provider.base_dir(&home);
        if base.is_dir() {
            accounts.push(base);
        }
        for account in accounts {
            crate::builtins::sync_finish_builtins(harness, &account)?;
        }
    }

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
        let files_pruned = |result: &Option<SyncResult>| {
            result.as_ref().map(|r| r.pruned_files.len()).unwrap_or(0)
        };
        let claude_files_pruned = files_pruned(&claude_result);
        let codex_files_pruned = files_pruned(&codex_result);
        let grok_files_pruned = files_pruned(&grok_result);
        println!(
            r#"{{"claude_present":{claude_present},"claude_builtins_updated":{claude_total},"claude_builtin_reference_conflicts":{claude_conflicts},"claude_skills_pruned":{claude_pruned_n},"claude_files_pruned":{claude_files_pruned},"codex_present":{codex_present},"codex_builtins_updated":{codex_total},"codex_builtin_reference_conflicts":{codex_conflicts},"codex_skills_pruned":{codex_pruned_n},"codex_files_pruned":{codex_files_pruned},"grok_present":{grok_present},"grok_builtins_updated":{grok_total},"grok_builtin_reference_conflicts":{grok_conflicts},"grok_skills_pruned":{grok_pruned_n},"grok_files_pruned":{grok_files_pruned}}}"#
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
        // cas-3af4: a legacy store (rules/entries from before the task store
        // and the migration ledger) is still ours. Install the missing base
        // tables so the migration chain can bring it current, instead of
        // calling it uninitialized, reporting the phase ok, and leaving the
        // skills phase to fail on a column a migration adds.
        let legacy: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('entries', 'rules')",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let table_count = if table_count < 3 && legacy > 0 && !args.dry_run {
            crate::migration::ensure_base_schemas(&conn)
                .context("install the base tables of a legacy store")?;
            3
        } else {
            table_count
        };
        drop(conn);
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
    let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
    render_check_report(
        &mut fmt,
        &CheckView {
            current_version,
            latest_version,
            binary_update_available,
            schema: schema_status.as_ref().map(|status| {
                (
                    status.current_version,
                    status.latest_version,
                    pending_migrations,
                )
            }),
        },
    )?;
    fmt.flush()?;
    Ok(())
}

/// What `cas update --check` learned, gathered so the render is a pure
/// function of data and can be pinned at a fixed width (cas-cli-craft).
struct CheckView<'a> {
    current_version: &'a str,
    latest_version: &'a str,
    binary_update_available: bool,
    /// `(applied, latest, pending)` schema versions; `None` when this
    /// directory has no Cassy store.
    schema: Option<(u32, u32, usize)>,
}

/// Verdict → two rows → one remedy. `cas update --check` is read in one
/// glance: line one says whether anything is pending, the rows say what.
fn render_check_report(fmt: &mut Formatter<'_>, view: &CheckView<'_>) -> io::Result<()> {
    let dot = Icons::separator_dot(fmt.unicode());
    let arrow = Icons::arrow_right(fmt.unicode());
    let pending = view.schema.map(|(_, _, pending)| pending).unwrap_or(0);
    let (verdict, word, detail) = match (view.binary_update_available, pending, view.schema) {
        (true, pending, _) => {
            let schema = if pending > 0 {
                format!(" {dot} {pending} migrations pending")
            } else {
                String::new()
            };
            (
                Verdict::Warning,
                "update available".to_string(),
                format!(
                    "{} {arrow} {}{schema}",
                    view.current_version, view.latest_version
                ),
            )
        }
        (false, pending, Some((applied, latest, _))) if pending > 0 => (
            Verdict::Warning,
            format!(
                "{pending} {} pending",
                if pending == 1 { "migration" } else { "migrations" }
            ),
            format!(
                "Cassy {} {dot} schema v{applied} {arrow} v{latest}",
                view.current_version
            ),
        ),
        (false, _, Some((applied, _, _))) => (
            Verdict::Ok,
            "up to date".to_string(),
            format!("Cassy {} {dot} schema v{applied}", view.current_version),
        ),
        (false, _, None) => (
            Verdict::Ok,
            "up to date".to_string(),
            format!(
                "Cassy {} {dot} no Cassy store in this directory",
                view.current_version
            ),
        ),
    };
    fmt.verdict(verdict, &word, &detail)?;
    fmt.rule()?;

    fmt.write_bold_plain(&format!("{:<10}", "Binary"))?;
    fmt.write_raw(&format!(
        "{} installed {dot} {} latest",
        view.current_version, view.latest_version
    ))?;
    fmt.newline()?;
    fmt.write_bold_plain(&format!("{:<10}", "Schema"))?;
    match view.schema {
        Some((applied, latest, pending)) if pending > 0 => {
            fmt.write_raw(&format!(
                "v{applied} applied {dot} v{latest} latest {dot} {pending} pending"
            ))?;
        }
        Some((applied, latest, _)) => {
            fmt.write_raw(&format!("v{applied} applied {dot} v{latest} latest"))?;
        }
        None => fmt.write_raw("not initialized in this directory (cas init)")?,
    }
    fmt.newline()?;

    if view.binary_update_available {
        fmt.newline()?;
        fmt.remedy(2, "cas update")?;
    } else if pending > 0 {
        fmt.newline()?;
        fmt.remedy(2, "cas update --schema-only")?;
    }
    if pending > 0 {
        fmt.receipt(&format!(
            "  cas update --dry-run previews the {pending} pending"
        ))?;
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn remove_quarantine_if_present(installed_binary: &Path) -> anyhow::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let path = CString::new(installed_binary.as_os_str().as_bytes())
        .context("installed binary path contains a NUL byte")?;
    // SAFETY: The pointers refer to valid NUL-terminated strings.
    let result = unsafe { libc::removexattr(path.as_ptr(), c"com.apple.quarantine".as_ptr(), 0) };
    if result == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ENOATTR) {
        return Ok(());
    }
    Err(error).with_context(|| {
        format!(
            "could not remove com.apple.quarantine from installed binary {}",
            installed_binary.display()
        )
    })
}

#[cfg(not(target_os = "macos"))]
fn remove_quarantine_if_present(_installed_binary: &Path) -> anyhow::Result<()> {
    Ok(())
}

/// Time from process creation through dyld and CLI dispatch to the post-swap
/// entry. The old updater cannot time this first launch, but its new child can
/// include the measurement in the refresh receipt the old parent merges.
#[cfg(target_os = "macos")]
fn post_swap_first_launch_ms() -> anyhow::Result<u64> {
    use std::mem::MaybeUninit;
    use std::time::{SystemTime, UNIX_EPOCH};

    let mut info = MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = std::mem::size_of::<libc::proc_bsdinfo>();
    // SAFETY: proc_pidinfo writes no more than the supplied buffer length.
    let written = unsafe {
        libc::proc_pidinfo(
            libc::getpid(),
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            size as i32,
        )
    };
    anyhow::ensure!(
        written == size as i32,
        "could not read installed binary process start time: {}",
        io::Error::last_os_error()
    );
    // SAFETY: A full proc_bsdinfo was written above.
    let info = unsafe { info.assume_init() };
    let started_us =
        u128::from(info.pbi_start_tvsec) * 1_000_000 + u128::from(info.pbi_start_tvusec);
    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_micros();
    Ok(now_us
        .saturating_sub(started_us)
        .div_ceil(1_000)
        .min(u64::MAX as u128) as u64)
}

#[cfg(not(target_os = "macos"))]
fn post_swap_first_launch_ms() -> anyhow::Result<u64> {
    Ok(0)
}

/// cas-cef2 (GH #1242): running factory sessions as `(name, pinned)`, where
/// `pinned` means the daemon pinned its own executable for provisioning.
fn running_factory_sessions() -> Vec<(String, bool)> {
    crate::ui::factory::SessionManager::new()
        .list_sessions_read_only()
        .unwrap_or_default()
        .into_iter()
        .filter(|session| session.is_running)
        .map(|session| {
            let pinned = crate::ui::factory::provisioner_pin_path(&session.name).is_file();
            (session.name, pinned)
        })
        .collect()
}

/// cas-cef2: what a binary update means for factory sessions already running.
/// A daemon keeps its own build until relaunched; one that pinned its
/// executable keeps spawning workers with it, and an older one cannot spawn.
fn running_factory_update_notice(sessions: &[(String, bool)], version: &str) -> Option<String> {
    if sessions.is_empty() {
        return None;
    }
    let names = |pinned: bool| {
        sessions
            .iter()
            .filter(|(_, is_pinned)| *is_pinned == pinned)
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut notice = format!(
        "{} running factory session(s) keep their current build until relaunched.",
        sessions.len()
    );
    let pinned = names(true);
    if !pinned.is_empty() {
        notice.push_str(&format!(" {pinned} keep spawning workers with that build."));
    }
    let unpinned = names(false);
    if !unpinned.is_empty() {
        notice.push_str(&format!(
            " {unpinned} started before build pinning and cannot spawn new workers until relaunched."
        ));
    }
    notice.push_str(&format!(
        " To move a session to {version}, run `cas factory kill <session>` and relaunch `cas factory`; this stops its workers."
    ));
    Some(notice)
}

/// Pre-swap failures have no refresh receipt. Preserve their original error
/// for stderr; post-swap failures already own a receipt claiming the new binary.
fn binary_install_failure(cli: &Cli, current: &str, error: anyhow::Error) -> anyhow::Error {
    if cli.json {
        let mut receipt = combined_update_receipt(current, false, None, None, None);
        receipt["binary_status"] = serde_json::json!("failed");
        receipt["refresh_status"] = serde_json::json!("not_run");
        receipt["error"] = serde_json::json!({"phase": "binary", "message": format!("{error:#}")});
        println!("{receipt}");
    }
    error
}

/// Download and install the latest (or specified) version
fn perform_update(
    args: &UpdateArgs,
    current_version: &str,
    cli: &Cli,
) -> anyhow::Result<BinaryUpdateOutcome> {
    use self_update::Status;
    use self_update::update::ReleaseUpdate;

    let updater = configure_updater(args, current_version, cli, io::stdin().is_terminal())
        .map_err(|error| binary_install_failure(cli, current_version, error))?;
    let updater = updater.build()
        .map_err(|error| binary_install_failure(cli, current_version, error.into()))?;
    // self_update resolves its install destination before the replacement.
    // Keep that stable path: after a Linux rename swap, current_exe() can
    // report the old process image as `/path/cas (deleted)`.
    let installed_binary = strip_deleted_suffix(updater.bin_install_path());

    // Check what we're updating to
    let latest = updater.get_latest_release()
        .map_err(|error| binary_install_failure(cli, current_version, error.into()))?;
    let target_version = args
        .version
        .as_ref()
        .map(|v| v.trim_start_matches('v').to_string())
        .unwrap_or_else(|| latest.version.trim_start_matches('v').to_string());

    // cas-70a8: the confirmation is CAS's own, in the phase's grammar.
    // self_update's prompt came with an unwrapped release-status block, and
    // its release check printed chatter around the phase line.
    if !update_interactivity(args, cli, io::stdin().is_terminal()).no_confirm {
        if !is_newer(&target_version, current_version) && args.version.is_none() {
            return Ok(BinaryUpdateOutcome {
                version: current_version.to_owned(),
                updated: false,
                post_install_refresh: None,
                pending_post_swap: None,
                notes: Vec::new(),
            });
        }
        confirm_install(current_version, &target_version)?;
    }

    // Perform the update. A run that asked first shows its download on the
    // phase line (cas-70a8): self_update's own bar ended without a newline,
    // so the phase result landed on the same line.
    let status = {
        let _download = live::during_open_phase(format!("downloading Cassy {target_version}"));
        updater.update()
    }
    .map_err(|error| binary_install_failure(
        cli,
        current_version,
        install_error(&error, current_version, &target_version),
    ))?;
    if matches!(&status, Status::Updated(_)) {
        // A new updater can remove quarantine before the first child launch.
        // An older parent cannot, so execute_post_swap repeats this before
        // relaunching the hub and leaves the installed binary clean.
        remove_quarantine_if_present(&installed_binary)?;
    }

    // cas-91ba: once the binary is swapped, THIS process is the old image.
    // Everything the new release changes — discovery, the user-level store,
    // migration repair — must run in the installed binary, so the remaining
    // phases are delegated to it and its receipt is the authoritative one.
    let mut post_install_refresh = None;
    let mut pending_post_swap = None;
    if matches!(&status, Status::Updated(_)) && !cli.json {
        // cas-0aa5: the child draws its own phase on this terminal, so it
        // runs after the binary phase has drawn its result line.
        pending_post_swap = Some(PendingPostSwap {
            installed_binary: installed_binary.clone(),
            installed_version: match &status {
                Status::UpToDate(v) | Status::Updated(v) => v.trim_start_matches('v').to_owned(),
            },
        });
    } else if matches!(&status, Status::Updated(_)) {
        let installed_version = match &status {
            Status::UpToDate(v) | Status::Updated(v) => v.trim_start_matches('v').to_owned(),
        };
        match run_post_swap_refresh(
            &installed_binary,
            current_version,
            &installed_version,
            cli.json,
        ) {
            Ok(receipt) => post_install_refresh = Some(receipt),
            Err(error) => {
                // The binary IS updated, but the phases that make the update
                // take effect did not run. Say so and fail: a partial update
                // must never read as converged to a script (cas-91ba).
                let reason = format!("{error:#}");
                if cli.json {
                    let receipt = error
                        .downcast_ref::<PostSwapRefreshFailure>()
                        .and_then(|failure| failure.receipt.clone())
                        .unwrap_or_else(|| skipped_refresh_receipt(&installed_version, &reason));
                    println!("{receipt}");
                }
                return Err(error);
            }
        }
    }

    if cli.json {
        let (updated, version) = match &status {
            Status::UpToDate(v) => (false, v.as_str()),
            Status::Updated(v) => (true, v.as_str()),
        };
        return Ok(BinaryUpdateOutcome {
            version: version.trim_start_matches('v').to_owned(),
            updated,
            post_install_refresh,
            pending_post_swap: None,
            notes: Vec::new(),
        });
    }

    let (updated, version) = match &status {
        Status::UpToDate(v) => (false, v.trim_start_matches('v').to_owned()),
        Status::Updated(v) => (true, v.trim_start_matches('v').to_owned()),
    };
    let notes = binary_notes(updated, &version, &running_factory_sessions());
    Ok(BinaryUpdateOutcome {
        version,
        updated,
        post_install_refresh,
        pending_post_swap,
        notes,
    })
}

/// A failed download or swap, in the reader's words: self_update's type
/// prefix (`UpdateError:`, `NetworkError:`) is dropped and the state of the
/// installed binary is said, so the next step is clear (cas-70a8).
fn install_error(error: &self_update::errors::Error, current: &str, target: &str) -> anyhow::Error {
    let text = error.to_string();
    let cause = match text.split_once(": ") {
        Some((kind, rest)) if kind.ends_with("Error") && !kind.contains(' ') => rest.to_owned(),
        _ => text,
    };
    // One clause per line when the reason passes 80 columns (exit_reason).
    anyhow::anyhow!(exit_reason(&[
        format!("could not install Cassy {target}"),
        cause,
        format!("Cassy {current} is unchanged; rerun `cas update` to retry"),
    ]))
}

/// Ask before replacing the binary: `Enter` or `y` installs, anything else
/// keeps the current binary, as self_update's own prompt did.
fn confirm_install(current: &str, target: &str) -> anyhow::Result<()> {
    let mut out = io::stdout();
    let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
    let arrow = Icons::arrow_right(fmt.unicode());
    fmt.write_raw("  ")?;
    fmt.mark(Verdict::Info)?;
    fmt.write_raw(" ")?;
    fmt.write_text(&format!("Cassy {current} {arrow} {target} is available"))?;
    fmt.newline()?;
    fmt.write_raw("    Download and install it now? [Y/n] ")?;
    fmt.flush()?;
    let mut answer = String::new();
    io::stdin().read_line(&mut answer)?;
    let answer = answer.trim().to_ascii_lowercase();
    if answer.is_empty() || answer == "y" || answer == "yes" {
        return Ok(());
    }
    Err(UpdateCancelled(current.to_owned()).into())
}

/// The operator declined the install; the current binary is untouched.
#[derive(Debug)]
struct UpdateCancelled(String);

impl std::fmt::Display for UpdateCancelled {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "update cancelled; Cassy {} is unchanged", self.0)
    }
}

impl std::error::Error for UpdateCancelled {}

/// What the binary phase adds below its result line: where to read about
/// the release, and what happens to running factories.
fn binary_notes(updated: bool, version: &str, factory_sessions: &[(String, bool)]) -> Vec<String> {
    if !updated {
        return Vec::new();
    }
    let mut notes = vec!["cas changelog shows what's new".to_owned()];
    notes.extend(running_factory_update_notice(factory_sessions, version));
    notes
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
struct UpdateInteractivity {
    no_confirm: bool,
    show_output: bool,
    show_download_progress: bool,
}

fn update_interactivity(
    args: &UpdateArgs,
    cli: &Cli,
    stdin_is_terminal: bool,
) -> UpdateInteractivity {
    let silent = cli.json || !stdin_is_terminal;
    let no_confirm = args.yes || silent;
    // cas-0aa5 / cas-70a8: the phase line says what self_update's chatter
    // said, and the confirmation is CAS's own (`confirm_install`), so the
    // release check never prints.
    UpdateInteractivity {
        no_confirm,
        show_output: false,
        // cas-70a8 QA F01: the phase line shows the download instead.
        show_download_progress: false,
    }
}

fn configure_updater(
    args: &UpdateArgs,
    current_version: &str,
    cli: &Cli,
    stdin_is_terminal: bool,
) -> anyhow::Result<self_update::backends::github::UpdateBuilder> {
    use self_update::backends::github::Update;

    if !args.yes && !cli.json && !stdin_is_terminal {
        anyhow::bail!("cas update requires --yes when stdin is not a TTY");
    }

    let interactivity = update_interactivity(args, cli, stdin_is_terminal);
    let mut updater = Update::configure();
    updater
        .repo_owner(REPO_OWNER)
        .repo_name(REPO_NAME)
        .bin_name(BIN_NAME)
        .current_version(current_version)
        .show_download_progress(interactivity.show_download_progress)
        .show_output(interactivity.show_output)
        // perform_update has already asked (confirm_install) when it must.
        .no_confirm(true);
    if let Some(token) = github_auth_token() {
        updater.auth_token(&token);
    }

    // Debug builds alone may point update E2E tests at a loopback release
    // server. Release builds cannot redirect binary downloads through env.
    #[cfg(debug_assertions)]
    if let Ok(api_url) = std::env::var("CAS_UPDATE_GITHUB_API_URL") {
        let parsed = url::Url::parse(&api_url).context("invalid CAS_UPDATE_GITHUB_API_URL")?;
        anyhow::ensure!(
            parsed.scheme() == "http"
                && matches!(parsed.host_str(), Some("127.0.0.1" | "::1" | "localhost"))
                && parsed.port().is_some()
                && parsed.username().is_empty()
                && parsed.password().is_none(),
            "CAS_UPDATE_GITHUB_API_URL must be an unauthenticated loopback HTTP URL with a port"
        );
        updater.with_url(api_url.trim_end_matches('/'));
    }

    if let Some(ref version) = args.version {
        updater.target_version_tag(&format!("v{}", version.trim_start_matches('v')));
    }

    Ok(updater)
}

/// What the binary phase produced, including the refresh the newly installed
/// binary already performed on our behalf (cas-91ba).
struct BinaryUpdateOutcome {
    version: String,
    updated: bool,
    /// `Some` when the installed binary ran the post-install phases. In
    /// `--json` it holds that child's receipt; otherwise `Null`, because the
    /// child printed its own human output.
    post_install_refresh: Option<serde_json::Value>,
    /// Human mode: the swap happened and the installed binary still has to
    /// run the remaining phases on this terminal, after the binary phase line.
    pending_post_swap: Option<PendingPostSwap>,
    /// Lines for below the binary phase's result, in human mode.
    notes: Vec<String>,
}

/// The installed binary that must run the post-swap phases.
struct PendingPostSwap {
    installed_binary: PathBuf,
    installed_version: String,
}

/// Run the post-swap phases in the installed binary, which renders them as
/// phase 2 of this update on the inherited terminal.
fn finish_post_swap(pending: &PendingPostSwap, current_version: &str) -> anyhow::Result<()> {
    run_post_swap_refresh(
        &pending.installed_binary,
        current_version,
        &pending.installed_version,
        false,
    )
    .map(|_| ())
}

/// A post-swap child started, but its refresh did not complete successfully.
///
/// Keep the receipt beside the error so the parent can still emit the
/// per-project diagnosis in `--json` mode. A plain `anyhow!` error loses that
/// distinction and makes the caller fall back to the misleading "skipped"
/// receipt (cas-c650).
#[derive(Debug)]
struct PostSwapRefreshFailure {
    receipt: Option<serde_json::Value>,
    message: String,
}

impl PostSwapRefreshFailure {
    fn new(receipt: serde_json::Value, message: String) -> Self {
        Self {
            receipt: Some(receipt),
            message,
        }
    }
}

impl std::fmt::Display for PostSwapRefreshFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for PostSwapRefreshFailure {}

fn execute_post_swap(args: &UpdateArgs, cli: &Cli, current_version: &str) -> anyhow::Result<()> {
    let first_launch_ms = post_swap_first_launch_ms()?;
    // Keep the old version available to the internal protocol and future
    // diagnostics without rendering it into the normal update receipt.
    let _previous_version = args
        .from
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("post-swap mode requires --from"))?;
    remove_quarantine_if_present(&std::env::current_exe()?)?;
    // The hub goes first so it picks up the new binary immediately rather than
    // waiting behind a full refresh. Its verification is drawn from data in
    // the findings; anything else it printed still reaches the operator.
    let (hub_restart, hub_output) =
        capture_phase(!cli.json, || super::hub::restart_stale_hub(current_version, cli));
    let hub_restart = hub_restart?;
    print_indented(&binary_phase_remainder(&hub_output, current_version, Some(&hub_restart)))?;

    // These are the phases that must not run in the pre-update image. To the
    // operator they are phase 2 of the update the parent started.
    let mut steps = UpdateStepTracker::resuming(1, 2, !cli.json);
    let hub_proof = hub_restart.clone();
    refresh_after_hub_restart(hub_restart, |error| {
        run_refresh_phase(&mut steps, cli, true, || {
            refresh_all_projects(
                args,
                cli,
                None,
                error,
                Some(&hub_proof),
                Some(first_launch_ms),
            )
        })
    })?;
    if hub_proof.action == "failed" {
        anyhow::bail!(
            "hub recovery failed after one retry: {}; {}",
            hub_proof.failure.as_deref().unwrap_or("unknown verification error"),
            hub_proof.remedy.as_deref().unwrap_or("Run `cas hub restart --force`."),
        );
    }
    Ok(())
}

fn build_post_swap_command(
    installed_binary: &Path,
    previous_version: &str,
    json: bool,
) -> std::process::Command {
    build_post_swap_command_with_receipt(installed_binary, previous_version, json, None)
}

fn build_post_swap_command_with_receipt(
    installed_binary: &Path,
    previous_version: &str,
    json: bool,
    receipt_path: Option<&Path>,
) -> std::process::Command {
    let mut command = std::process::Command::new(installed_binary);
    command.args(["update", "--post-swap", "--from"]);
    command.arg(previous_version);
    if json {
        command.arg("--json");
    }
    if let Some(receipt_path) = receipt_path {
        command.args(["--refresh-receipt"]);
        command.arg(receipt_path);
    }
    command
}

/// Run the post-install phases with the freshly installed binary (cas-91ba).
///
/// The phases after the swap — schema migrations, the all-projects refresh,
/// the user-level store and skills sync — are exactly the behaviour the new
/// release changes, so running them in the pre-update process image reports
/// the old version's results and forces the operator to run `cas update`
/// twice. The child is the newly installed binary, so its receipt is the
/// authoritative one.
///
/// In `--json` the child's stdout is captured and returned so the parent can
/// emit ONE combined document; otherwise the child inherits stdio and the
/// operator watches the phases live.
fn run_post_swap_refresh(
    installed_binary: &Path,
    previous_version: &str,
    installed_version: &str,
    json: bool,
) -> anyhow::Result<serde_json::Value> {
    let receipt_file = tempfile::NamedTempFile::new()
        .context("could not create a post-swap refresh receipt file")?;
    let receipt_path = receipt_file.path().to_owned();
    let mut command = build_post_swap_command_with_receipt(
        installed_binary,
        previous_version,
        json,
        Some(&receipt_path),
    );
    let rerun_hint = post_swap_rerun_hint(installed_version, installed_binary);

    if !json {
        let status = match command.status() {
            Ok(status) => status,
            Err(error) => {
                let message =
                    format!("{rerun_hint} (could not start the installed binary): {error}");
                return Err(PostSwapRefreshFailure::new(
                    refresh_spawn_failure_receipt(installed_version, &message),
                    message,
                )
                .into());
            }
        };
        if !status.success() {
            let child_receipt = read_refresh_receipt(&receipt_path);
            let Some(child_receipt) = child_receipt else {
                let message = post_swap_refresh_unreported_hint(installed_version, status, "");
                return Err(PostSwapRefreshFailure::new(
                    refresh_failed_without_receipt(
                        installed_version,
                        "refresh_failed_no_receipt",
                        &message,
                    ),
                    message,
                )
                .into());
            };
            verify_child_refresh_receipt(&child_receipt, installed_version, installed_binary)?;
            let message =
                post_swap_refresh_failed_hint(installed_version, status, "", Some(&child_receipt));
            return Err(PostSwapRefreshFailure::new(
                refresh_failed_receipt(installed_version, child_receipt, status, "", &message),
                message,
            )
            .into());
        }
        // The child inherited stdio, so use its side-channel receipt when
        // available; retain the version probe for older installed children.
        if let Some(child_receipt) = read_refresh_receipt(&receipt_path) {
            verify_child_refresh_receipt(&child_receipt, installed_version, installed_binary)?;
        } else {
            verify_refresh_binary_version(
                reported_binary_version(installed_binary).as_deref(),
                installed_version,
                installed_binary,
            )?;
        }
        return Ok(serde_json::Value::Null);
    }

    let output = match command.output() {
        Ok(output) => output,
        Err(error) => {
            let message = format!("{rerun_hint} (could not start the installed binary): {error}");
            return Err(PostSwapRefreshFailure::new(
                refresh_spawn_failure_receipt(installed_version, &message),
                message,
            )
            .into());
        }
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let Some(child_receipt) =
            parse_refresh_receipt(&stdout).or_else(|| read_refresh_receipt(&receipt_path))
        else {
            let message =
                post_swap_refresh_unreported_hint(installed_version, output.status, stderr.trim());
            return Err(PostSwapRefreshFailure::new(
                refresh_failed_without_receipt(
                    installed_version,
                    "refresh_failed_no_receipt",
                    &message,
                ),
                message,
            )
            .into());
        };
        verify_child_refresh_receipt(&child_receipt, installed_version, installed_binary)?;
        let message = post_swap_refresh_failed_hint(
            installed_version,
            output.status,
            stderr.trim(),
            Some(&child_receipt),
        );
        let receipt = refresh_failed_receipt(
            installed_version,
            child_receipt,
            output.status,
            stderr.trim(),
            &message,
        );
        return Err(PostSwapRefreshFailure::new(receipt, message).into());
    }
    // The child may print progress lines before its receipt; the receipt is
    // the last JSON document on stdout.
    let receipt = parse_refresh_receipt(&stdout)
        .or_else(|| read_refresh_receipt(&receipt_path))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "{rerun_hint} (its output carried no JSON receipt: {})",
                stdout.trim()
            )
        })?;
    verify_child_refresh_receipt(&receipt, installed_version, installed_binary)?;
    Ok(receipt)
}

/// The child may print progress lines before its machine-readable receipt.
/// Only object receipts can prove that the refresh path ran.
fn parse_refresh_receipt(stdout: &str) -> Option<serde_json::Value> {
    stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<serde_json::Value>(line.trim()).ok())
        .filter(|receipt| receipt.is_object() && receipt.get("refresh_binary_version").is_some())
}

fn read_refresh_receipt(path: &Path) -> Option<serde_json::Value> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(contents.trim())
        .ok()
        .filter(|receipt: &serde_json::Value| {
            receipt.is_object() && receipt.get("refresh_binary_version").is_some()
        })
}

fn post_swap_refresh_failed_hint(
    installed_version: &str,
    status: std::process::ExitStatus,
    stderr: &str,
    receipt: Option<&serde_json::Value>,
) -> String {
    let stderr = stderr.trim();
    let failure_details = receipt.and_then(refresh_failure_details);
    let diagnosis = failure_details.map_or_else(
        || {
            "inspect the per-project refresh results and resolve the reported failures before rerunning `cas update --all-projects`".to_owned()
        },
        |details| {
            format!(
                "failed project(s): {details}; resolve the reported project/cloud errors before rerunning `cas update --all-projects`"
            )
        },
    );
    if stderr.is_empty() {
        format!(
            "binary updated to {installed_version}; post-swap refresh ran and failed (exit status {status}); {diagnosis}"
        )
    } else {
        format!(
            "binary updated to {installed_version}; post-swap refresh ran and failed (exit status {status}): {stderr}; {diagnosis}"
        )
    }
}

fn refresh_failure_details(receipt: &serde_json::Value) -> Option<String> {
    let projects = receipt.get("projects")?.as_array()?;
    let mut failures = Vec::new();
    for project in projects {
        let Some(name) = project.get("project").and_then(|value| value.as_str()) else {
            continue;
        };
        let phases = [
            ("migration", project.get("migration")),
            ("search_index", project.get("search_index")),
            ("skills", project.get("skills")),
            ("membership", project.get("membership")),
            ("cloud_sync", project.get("cloud_sync")),
        ];
        let failed_phases = phases
            .into_iter()
            .filter_map(|(phase, value)| {
                let detail = value?.as_str()?;
                detail
                    .starts_with("FAILED:")
                    .then(|| format!("{phase}: {detail}"))
            })
            .collect::<Vec<_>>();
        if !failed_phases.is_empty() {
            failures.push(format!("{name} ({})", failed_phases.join(", ")));
        }
    }
    (!failures.is_empty()).then(|| failures.join("; "))
}

fn post_swap_refresh_unreported_hint(
    installed_version: &str,
    status: std::process::ExitStatus,
    stderr: &str,
) -> String {
    let stderr = stderr.trim();
    if stderr.is_empty() {
        format!(
            "binary updated to {installed_version}; post-swap child exited with status {status} without a refresh receipt; the refresh outcome is unknown — inspect the child diagnostics before rerunning `cas update --all-projects`"
        )
    } else {
        format!(
            "binary updated to {installed_version}; post-swap child exited with status {status} without a refresh receipt: {stderr}; the refresh outcome is unknown — inspect the child diagnostics before rerunning `cas update --all-projects`"
        )
    }
}

fn refresh_failed_receipt(
    installed_version: &str,
    child_receipt: serde_json::Value,
    status: std::process::ExitStatus,
    stderr: &str,
    message: &str,
) -> serde_json::Value {
    let mut receipt = match child_receipt {
        serde_json::Value::Object(receipt) => receipt,
        _ => {
            return refresh_failed_without_receipt(
                installed_version,
                "refresh_failed_no_receipt",
                message,
            );
        }
    };
    receipt.insert("binary_updated".to_owned(), serde_json::Value::Bool(true));
    receipt.insert(
        "version".to_owned(),
        serde_json::Value::String(installed_version.to_owned()),
    );
    receipt.insert(
        "refresh_status".to_owned(),
        serde_json::Value::String("refresh_failed".to_owned()),
    );
    receipt.insert(
        "refresh_exit_status".to_owned(),
        serde_json::Value::String(status.to_string()),
    );
    if !stderr.trim().is_empty() {
        receipt.insert(
            "refresh_stderr".to_owned(),
            serde_json::Value::String(stderr.trim().to_owned()),
        );
    }
    receipt.insert(
        "message".to_owned(),
        serde_json::Value::String(message.to_owned()),
    );
    serde_json::Value::Object(receipt)
}

fn refresh_spawn_failure_receipt(installed_version: &str, message: &str) -> serde_json::Value {
    serde_json::json!({
        "binary_updated": true,
        "version": installed_version,
        "refresh_binary_version": serde_json::Value::Null,
        "refresh_status": "spawn_failed",
        "message": message,
    })
}

fn refresh_failed_without_receipt(
    installed_version: &str,
    status: &str,
    message: &str,
) -> serde_json::Value {
    serde_json::json!({
        "binary_updated": true,
        "version": installed_version,
        "refresh_binary_version": serde_json::Value::Null,
        "refresh_status": status,
        "message": message,
    })
}

/// `<binary> --version` reduced to the bare semver, or None when it cannot be
/// asked.
fn reported_binary_version(binary: &Path) -> Option<String> {
    let output = std::process::Command::new(binary).arg("--version").output().ok()?;
    if !output.status.success() {
        return None;
    }
    parse_reported_version(&String::from_utf8_lossy(&output.stdout))
}

/// The semver inside a `--version` line, or None when there is none.
///
/// Clap prints `cas <semver> (<hash>[-dirty] <date>)`, so the last whitespace
/// token is the build date (`2026-09-04)`), not a version. Take the first token
/// that is actually a semver instead, with the optional `v` prefix stripped.
fn parse_reported_version(output: &str) -> Option<String> {
    output.split_whitespace().find_map(|token| {
        let candidate = token.strip_prefix('v').unwrap_or(token);
        is_semver_token(candidate).then(|| candidate.to_string())
    })
}

/// `major.minor.patch` in digits, plus an optional `-pre`/`+build` suffix.
fn is_semver_token(token: &str) -> bool {
    let core_end = token.find(['-', '+']).unwrap_or(token.len());
    let (core, suffix) = token.split_at(core_end);

    let mut parts = core.split('.');
    let core_is_semver = match (parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(major), Some(minor), Some(patch), None) => [major, minor, patch]
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit())),
        _ => false,
    };
    if !core_is_semver {
        return false;
    }

    // A suffix must carry something after its `-`/`+` and stay inside the
    // characters semver allows, so `3.15.5-` or `3.15.5-(x)` do not pass.
    suffix.is_empty()
        || (suffix.len() > 1
            && suffix[1..]
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-' || b == b'+'))
}

/// The operator-facing line for a swap whose post-install phases did not run.
fn post_swap_rerun_hint(installed_version: &str, installed_binary: &Path) -> String {
    format!(
        "binary updated to {installed_version}; refresh did not run — run `cas update` again \
         (the installed binary at {} could not perform the post-install phases)",
        installed_binary.display()
    )
}

/// The parent's startup reads precede the child process's repair. Resolve
/// only maintenance paths the verified child actually represented, including
/// reported failures. Absent/unverified receipts leave notices for final replay.
fn verify_child_refresh_receipt(
    receipt: &serde_json::Value,
    installed_version: &str,
    installed_binary: &Path,
) -> anyhow::Result<()> {
    verify_refresh_binary_version(
        receipt
            .get("refresh_binary_version")
            .and_then(|v| v.as_str()),
        installed_version,
        installed_binary,
    )?;
    let projects = receipt
        .get("projects")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|project| project.get("retired_toml"));
    let user = receipt
        .pointer("/user_level_store/retired_toml")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten();
    for maintenance in projects.chain(user) {
        let represented = match maintenance.get("status").and_then(|v| v.as_str()) {
            Some("repaired" | "unchanged" | "missing" | "planned") => true,
            Some("failed") => maintenance.get("error").and_then(|v| v.as_str()).is_some(),
            _ => false,
        };
        if !represented {
            continue;
        }
        if let Some(path) = maintenance
            .get("path")
            .and_then(|v| v.as_str())
            .map(Path::new)
            && path.is_absolute()
            && path.file_name() == Some(std::ffi::OsStr::new("config.toml"))
            && let Some(root) = path.parent()
        {
            crate::config::represented_retired_config(root);
        }
    }
    Ok(())
}

/// The child must be the binary we just installed. A version it does not
/// report — or reports differently — means something else answered (a stale
/// `cas` earlier on PATH, a wrapper script), and its refresh proves nothing.
fn verify_refresh_binary_version(
    reported: Option<&str>,
    installed_version: &str,
    installed_binary: &Path,
) -> anyhow::Result<()> {
    let reported = reported.map(str::trim).unwrap_or_default();
    if reported == installed_version.trim() {
        return Ok(());
    }
    anyhow::bail!(
        "the refresh ran as version {reported:?} but {} was just installed as {installed_version}; \
         refusing to report a converged update — run `cas update` again",
        installed_binary.display()
    )
}

/// The receipt for a swap whose post-install phases could not run (cas-91ba).
fn skipped_refresh_receipt(installed_version: &str, reason: &str) -> serde_json::Value {
    serde_json::json!({
        "binary_updated": true,
        "version": installed_version,
        "refresh_binary_version": serde_json::Value::Null,
        "refresh_status": "skipped",
        "message": reason,
    })
}

pub(super) fn strip_deleted_suffix(path: std::path::PathBuf) -> std::path::PathBuf {
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

/// Numbered phases of one `cas update` run, each drawn as one phase line
/// (cas-0aa5; see `update/live.rs`).
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

    /// Continue a run another process started: the post-swap child is the
    /// second phase of its parent's update.
    fn resuming(done: usize, total: usize, enabled: bool) -> Self {
        Self {
            total,
            current: done,
            enabled,
        }
    }

    /// A phase whose result is simply "done": its own output follows its line.
    fn run<T, F>(&mut self, label: &str, f: F) -> anyhow::Result<T>
    where
        F: FnOnce() -> anyhow::Result<T>,
    {
        self.phase(
            label,
            "working",
            true,
            f,
            |result| live::PhaseResult {
                verdict: if result.is_ok() { Verdict::Ok } else { Verdict::Error },
                text: if result.is_ok() { "done" } else { "failed" }.to_owned(),
            },
            |_, output| output.to_owned(),
        )
    }

    /// Run one phase on its line. With `capture`, everything the phase prints
    /// is held until its result is drawn, then `keep` chooses what still adds
    /// to that result. Without it (an interactive prompt needs the terminal)
    /// the phase prints freely between its start and result lines.
    fn phase<T>(
        &mut self,
        label: &str,
        doing: &str,
        capture: bool,
        f: impl FnOnce() -> anyhow::Result<T>,
        result: impl FnOnce(&anyhow::Result<T>) -> live::PhaseResult,
        keep: impl FnOnce(&anyhow::Result<T>, &str) -> String,
    ) -> anyhow::Result<T> {
        self.current += 1;
        if !self.enabled {
            return f();
        }
        let counter = format!("[{}/{}]", self.current, self.total);
        let started_at = Instant::now();
        let mode = OutputMode::detect();
        let animate = capture && live::can_animate();
        if !animate {
            print_phase_line(None, &counter, label, doing, None)?;
        }
        let (value, output) = if capture {
            let live_line = animate
                .then(terminal_handle)
                .flatten()
                .map(|terminal| {
                    live::LiveLine::start(
                        terminal,
                        mode,
                        counter.clone(),
                        label.to_owned(),
                        doing.to_owned(),
                    )
                });
            let (value, output) = capture_phase(true, f);
            if let Some(line) = live_line {
                line.finish();
            }
            (value, output)
        } else {
            live::set_open_phase(Some((mode, counter.clone(), label.to_owned())));
            let value = f();
            live::set_open_phase(None);
            (value, String::new())
        };
        let shown = result(&value);
        print_phase_line(
            Some(shown.verdict),
            &counter,
            label,
            &shown.text,
            Some(started_at.elapsed()),
        )?;
        let kept = keep(&value, &output);
        print_indented(&kept)?;
        value
    }
}

/// The real terminal, held open while the phase's own stdout is captured.
pub(super) fn terminal_handle() -> Option<std::fs::File> {
    #[cfg(unix)]
    {
        use std::os::fd::FromRawFd;
        let fd = unsafe { libc::dup(libc::STDOUT_FILENO) };
        (fd >= 0).then(|| unsafe { std::fs::File::from_raw_fd(fd) })
    }
    #[cfg(not(unix))]
    {
        None
    }
}

fn print_phase_line(
    mark: Option<Verdict>,
    counter: &str,
    label: &str,
    text: &str,
    elapsed: Option<Duration>,
) -> io::Result<()> {
    let mut out = io::stdout();
    let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
    live::render_line(&mut fmt, mark, None, counter, label, text, elapsed)?;
    fmt.newline()?;
    fmt.flush()
}

/// Output a phase printed that its result line does not already say.
fn print_indented(output: &str) -> io::Result<()> {
    let mut out = io::stdout();
    for line in output.lines().filter(|line| !line.trim().is_empty()) {
        writeln!(out, "  {}", line.trim_end())?;
    }
    out.flush()
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

fn report_managed_files(
    cli: &Cli,
    report: &crate::cli::project_files::Report,
) -> anyhow::Result<()> {
    if cli.json {
        println!("{}", serde_json::json!({"managed_files": report}));
    } else {
        let message = if report.commit_error.is_some() {
            "Managed files: commit failed (details below)".to_string()
        } else if !report.conflicts.is_empty() {
            format!(
                "Managed files: {} conflicts preserved; cas update --sync --json for paths",
                report.conflicts.len()
            )
        } else if report.outcome == "staged" {
            "Managed files: staged; review with git diff --cached".to_string()
        } else {
            format!(
                "Managed files: {} ({} untracked, {} staged)",
                report.outcome, report.untracked, report.staged
            )
        };
        println!("{message}");
    }
    Ok(())
}
