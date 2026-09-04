//! Cloud sync commands for Cassy
//!
//! Enables syncing Cassy data with Cassy Cloud service.

use clap::{Args, Parser, Subcommand};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::io::IsTerminal;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::cli::Cli;
use crate::cloud::{
    BackfillOutcome, CloudConfig, CloudSyncerConfig, FetchTeamsOutcome, PersonalScopeNotice,
    SyncQueue, TeamInfo, fetch_and_cache_teams, maybe_apply_team_backfill,
    maybe_mark_personal_scope_notice, teams_cache_stale, user_level_cloud_json_path,
};
use crate::ui::components::Formatter;
use crate::ui::theme::ActiveTheme;

use crate::store::{
    AgentStore, open_agent_store, open_commit_link_store, open_event_store, open_file_change_store,
    open_prompt_store, open_rule_store_local, open_skill_store_local, open_spec_store,
    open_store_local, open_task_store_local,
};

#[derive(Subcommand)]
pub enum CloudCommands {
    /// Show cloud sync status
    Status,
    /// Show sync queue (pending changes)
    Queue(CloudQueueArgs),
    /// List locally retained pull-side conflicts, or prune old records
    Conflicts(CloudConflictsArgs),
    /// Push local data to cloud
    Push(CloudPushArgs),
    /// Pull data from cloud
    Pull(CloudPullArgs),
    /// Full sync (push then pull)
    Sync(CloudSyncArgs),
    /// Configure the active team for team-scoped sync operations
    #[command(subcommand)]
    Team(CloudTeamCommands),
    /// Configure the project canonical slug (overrides auto-derivation)
    Project(CloudProjectArgs),
    /// List team projects in cloud
    Projects(CloudProjectsArgs),
    /// Pull team memories for the current project
    TeamMemories(CloudTeamMemoriesArgs),
    /// Remove this project's local cloud link, optionally purging owned cloud rows
    Unlink(CloudUnlinkArgs),
    /// Remove foreign-project entities from local DB and re-pull
    PurgeForeign(CloudPurgeForeignArgs),
}

/// Subcommands for `cas cloud team`
#[derive(Subcommand)]
pub enum CloudTeamCommands {
    /// Set the user-level default team (resolves slug or UUID against cached memberships)
    ///
    /// Writes `default_team_id` to `~/.cas/cloud.json`. All projects without an
    /// explicit per-project team override (`cas cloud team set`) will use this
    /// default for team-scoped sync.
    ///
    /// Use `--personal` to revert to personal scope (clears the default).
    ///
    /// Requires cached team memberships — run `cas cloud login` first if you see
    /// "team not found" errors.
    Default(CloudTeamDefaultArgs),
    /// Set the per-project team override by slug or UUID
    ///
    /// Writes `team_id` to `<project>/.cas/cloud.json`. Slugs are resolved
    /// against cached memberships from `~/.cas/cloud.json`.
    Set(CloudTeamSetArgs),
    /// Configure whether this project inherits the user-level team default
    #[command(subcommand)]
    Auto(CloudTeamAutoCommands),
    /// Show the currently configured team
    Show,
    /// Clear the configured team (no more team-scoped sync)
    Clear,
}

/// Arguments for `cas cloud team default`.
#[derive(Parser)]
pub struct CloudTeamDefaultArgs {
    /// Team slug or UUID to set as the user-level default.
    ///
    /// Resolved against the cached team memberships in `~/.cas/cloud.json`.
    /// Omit when using `--personal`.
    #[arg(required_unless_present = "personal")]
    pub slug_or_uuid: Option<String>,

    /// Clear the default team and revert to personal scope.
    #[arg(long, conflicts_with = "slug_or_uuid")]
    pub personal: bool,
}

#[derive(Parser)]
pub struct CloudTeamSetArgs {
    /// Team slug or UUID (e.g., petra-stella or 550e8400-e29b-41d4-a716-446655440000)
    pub id: Option<String>,
}

/// Subcommands for `cas cloud team auto`.
#[derive(Subcommand)]
pub enum CloudTeamAutoCommands {
    /// Inherit the user-level default team for this project
    On,
    /// Disable team scope for this project, even if a team_id is configured
    Off,
    /// Clear the auto-promotion override
    Clear,
}

/// Subcommands for `cas cloud project` (cas-1ced).
///
/// Manual override for the project canonical slug used to scope cloud-sync
/// pushes/pulls. Normally `cas cloud team set` auto-derives the slug from
/// `.cas/config.toml` then from the git remote — use this subcommand when
/// auto-derivation fails (monorepo, custom layout, non-git checkout).
#[derive(Subcommand)]
pub enum CloudProjectCommands {
    /// Set the project canonical id (writes `.cas/config.toml [project] canonical_id`)
    Set(CloudProjectSetArgs),
    /// Rewrite local task origins that are aliases of this project's identity
    /// and enqueue canonical upserts for the next cloud sync.
    AdoptAliases,
}

/// Arguments for `cas cloud project`.
///
/// The alias-repair action is deliberately a flag so doctor can print one
/// copy/pasteable command that does not require a positional project id.
#[derive(Args)]
pub struct CloudProjectArgs {
    /// Rewrite local task origins that are aliases of this project's identity
    /// and enqueue canonical upserts for the next cloud sync.
    #[arg(long)]
    pub adopt_aliases: bool,

    /// Optional project subcommand (`set` or the legacy `adopt-aliases` form).
    #[command(subcommand)]
    pub command: Option<CloudProjectCommands>,
}

#[derive(Parser)]
pub struct CloudProjectSetArgs {
    /// Canonical project slug (e.g., `github.com/foo/bar`)
    pub canonical_id: String,
}

fn parse_positive_usize(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("expected a positive integer, got {value:?}"))?;
    if parsed == 0 {
        return Err("expected a positive integer, got 0".to_string());
    }
    Ok(parsed)
}

#[derive(Parser)]
pub struct CloudPushArgs {
    /// Push only entries
    #[arg(long, conflicts_with = "tasks_only")]
    pub entries_only: bool,

    /// Push only tasks
    #[arg(long, conflicts_with = "entries_only")]
    pub tasks_only: bool,

    /// Dry run (don't actually push)
    #[arg(long)]
    pub dry_run: bool,

    /// Stop after this many queue batches instead of draining the full backlog.
    #[arg(long, value_parser = parse_positive_usize)]
    pub max_batches: Option<usize>,

    /// Allow re-homing existing cloud entities to the current project slug.
    ///
    /// By default, `cas cloud push` refuses when the `project_canonical_id` has
    /// changed since the last push — a changed slug would silently move all
    /// existing cloud entities into the new bucket (defect D from the ozer
    /// cloud-sync bug report). Pass `--rehome` to explicitly confirm the
    /// operation. Only needed when you intentionally changed the project slug
    /// via `cas cloud project set`.
    #[arg(long)]
    pub rehome: bool,
}

#[derive(Parser)]
pub struct CloudPullArgs {
    /// Pull only entries
    #[arg(long)]
    pub entries_only: bool,

    /// Pull only tasks
    #[arg(long)]
    pub tasks_only: bool,

    /// Pull all data (ignore last sync time)
    #[arg(long)]
    pub full: bool,
}

#[derive(Parser)]
pub struct CloudSyncArgs {
    /// Dry run (don't actually sync)
    #[arg(long)]
    pub dry_run: bool,

    /// Ignore pull watermarks and re-pull all data.
    #[arg(long)]
    pub full: bool,

    /// Allow re-homing existing cloud entities (passed through to push).
    ///
    /// See `cas cloud push --rehome` for details.
    #[arg(long)]
    pub rehome: bool,
}

#[derive(Parser)]
pub struct CloudProjectsArgs {
    /// Team UUID override (defaults to the team configured via `cas cloud team set`)
    #[arg(long)]
    pub team: Option<String>,
}

#[derive(Parser)]
pub struct CloudTeamMemoriesArgs {
    /// Show what would be pulled without merging
    #[arg(long)]
    pub dry_run: bool,

    /// Ignore last sync timestamp, pull everything
    #[arg(long)]
    pub full: bool,
}

#[derive(Parser)]
pub struct CloudPurgeForeignArgs {
    /// Preview what would be purged without deleting
    #[arg(long)]
    pub dry_run: bool,

    /// Proceed even when the recoverability guard refuses (stale pull state, or
    /// local rows that were never pushed to cloud). Classifier hard stops for a
    /// task majority or proven rule cannot be overridden. Destructive — the
    /// refusal reason is still printed.
    #[arg(long)]
    pub force: bool,

    /// Allow a purge whose classified foreign tasks are a majority of local
    /// tasks. This lifts only the ratio guard and always requires --yes.
    #[arg(long)]
    pub allow_majority_foreign: bool,

    /// Confirm the explicit majority-foreign override.
    #[arg(long)]
    pub yes: bool,

    /// Age in days beyond which the last successful cloud pull is considered
    /// stale and the purge is refused without --force.
    #[arg(long, default_value_t = PURGE_STALE_THRESHOLD_DAYS)]
    pub stale_days: i64,
}

#[derive(Parser)]
pub struct CloudUnlinkArgs {
    /// Also remove this project's entries, tasks, and knowledge rows from cloud.
    ///
    /// The remote purge is deliberately explicit. It uses only the existing
    /// per-owner DELETE endpoints and leaves the local database untouched.
    #[arg(long)]
    pub purge_remote: bool,

    /// Show the scoped remote rows that would be deleted without changing state.
    #[arg(long)]
    pub dry_run: bool,
}

#[derive(Parser)]
pub struct CloudQueueArgs {
    /// Show detailed list of queued items
    #[arg(long, short)]
    pub verbose: bool,

    /// Maximum items to show
    #[arg(long, default_value = "20")]
    pub limit: usize,

    /// Clear failed items older than N days
    #[arg(long)]
    pub prune: Option<i64>,

    /// Requeue all terminally failed items, preserving their last error.
    /// Combine with --retry-reason to target only diagnostics containing a
    /// repaired server reason.
    #[arg(long, conflicts_with_all = ["prune", "clear"])]
    pub retry: bool,

    /// Requeue only terminal items whose diagnostic contains this reason.
    #[arg(long, alias = "reason", requires = "retry")]
    pub retry_reason: Option<String>,

    /// Clear all items from the queue
    #[arg(long)]
    pub clear: bool,
}

#[derive(Parser)]
pub struct CloudConflictsArgs {
    /// Maximum conflict rows to show
    #[arg(long, default_value = "20")]
    pub limit: usize,

    /// Delete retained rows older than N days
    #[arg(long)]
    pub prune: Option<i64>,
}

pub fn execute(cmd: &CloudCommands, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    match cmd {
        CloudCommands::Status => execute_status(cli, cas_root),
        CloudCommands::Queue(args) => execute_queue(args, cli, cas_root),
        CloudCommands::Conflicts(args) => execute_conflicts(args, cli, cas_root),
        CloudCommands::Push(args) => execute_push(args, cli, cas_root).map(|_| ()),
        CloudCommands::Pull(args) => execute_pull(args, cli, cas_root).map(|_| ()),
        CloudCommands::Sync(args) => execute_sync(args, cli, cas_root),
        CloudCommands::Team(cmd) => execute_team(cmd, cli, cas_root),
        CloudCommands::Project(args) => {
            if args.adopt_aliases {
                execute_project_adopt_aliases(cli, cas_root)
            } else if let Some(cmd) = args.command.as_ref() {
                execute_project(cmd, cli, cas_root)
            } else {
                anyhow::bail!("`cas cloud project` requires a subcommand or --adopt-aliases")
            }
        }
        CloudCommands::Projects(args) => execute_projects(args, cli, cas_root),
        CloudCommands::TeamMemories(args) => execute_team_memories(args, cli, cas_root),
        CloudCommands::Unlink(args) => execute_unlink(args, cli, cas_root),
        CloudCommands::PurgeForeign(args) => execute_purge_foreign(args, cli, cas_root),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct UnlinkRemoteRecord {
    scope: UnlinkRemoteScope,
    entity_type: String,
    id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum UnlinkRemoteScope {
    Personal,
    Team(String),
}

/// Remove the current project's cloud link without touching any other local
/// `.cas` state. Remote deletion is a separate, explicit phase: all scoped
/// rows are discovered first, unsupported knowledge deletion fails closed, and
/// the local link is removed only after every DELETE succeeds.
fn execute_unlink(args: &CloudUnlinkArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let cloud_path = cas_root.join("cloud.json");
    if !cloud_path.exists() {
        return render_unlink_result(cli, cas_root, None, args, 0, 0, false, "not cloud-linked");
    }

    let mut config = CloudConfig::load_from_cas_dir(cas_root)?;
    // Login credentials are user-scoped. Inherit them in memory only: this
    // command must not rewrite cloud.json before the remote purge succeeds.
    if !config.is_logged_in() {
        if let Ok(user_config) = CloudConfig::load_user() {
            config.inherit_credentials_from(&user_config);
        }
    }
    let project_id = crate::cloud::resolve_canonical_id_for_sync(cas_root)
        .map_err(|error| anyhow::anyhow!("Cannot unlink: {error}"))?;

    if args.purge_remote {
        let token = config
            .token
            .as_deref()
            .filter(|token| !token.is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Cannot purge remote rows: not logged in. The local cloud link was preserved."
                )
            })?;
        let queue = SyncQueue::open_read_only(cas_root)?;
        let syncer = crate::cloud::CloudSyncer::new_for_project(
            std::sync::Arc::new(queue),
            config.clone(),
            crate::cloud::CloudSyncerConfig::default(),
            project_id.clone(),
            cas_root,
        );
        let records = discover_unlink_remote_records(
            &syncer,
            &project_id,
            config.active_team_id().as_deref(),
        )?;
        let knowledge_records = records
            .iter()
            .filter(|record| record.entity_type == "knowledge_page")
            .collect::<Vec<_>>();
        if !knowledge_records.is_empty() {
            let ids = knowledge_records
                .iter()
                .map(|record| record.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            anyhow::bail!(
                "Cannot purge remote knowledge pages: the cloud API does not support knowledge_page DELETE ({} row(s): {ids}). The local cloud link was preserved.",
                knowledge_records.len()
            );
        }
        if args.dry_run {
            return render_unlink_result(
                cli,
                cas_root,
                Some(&project_id),
                args,
                records.len(),
                0,
                false,
                "dry run",
            );
        }
        let deleted = delete_unlink_remote_records(&config.endpoint, token, &project_id, &records)?;
        fs::remove_file(&cloud_path).map_err(|error| {
            anyhow::anyhow!(
                "Remote purge completed ({deleted} row(s)), but removing {} failed: {error}",
                cloud_path.display()
            )
        })?;
        return render_unlink_result(
            cli,
            cas_root,
            Some(&project_id),
            args,
            records.len(),
            deleted,
            true,
            "cloud link removed; local data preserved",
        );
    }

    if args.dry_run {
        return render_unlink_result(
            cli,
            cas_root,
            Some(&project_id),
            args,
            0,
            0,
            false,
            "dry run; remote rows retained",
        );
    }
    fs::remove_file(&cloud_path)?;
    render_unlink_result(
        cli,
        cas_root,
        Some(&project_id),
        args,
        0,
        0,
        true,
        "cloud link removed; remote rows retained",
    )
}

fn discover_unlink_remote_records(
    syncer: &crate::cloud::CloudSyncer,
    project_id: &str,
    team_id: Option<&str>,
) -> anyhow::Result<Vec<UnlinkRemoteRecord>> {
    let mut records = BTreeSet::new();
    let entity_types = ["entries", "tasks", "knowledge_pages"];
    let personal = syncer
        .pull_raw(project_id, &entity_types, None)
        .map_err(|error| anyhow::anyhow!("personal cloud pull failed: {error}"))?;
    collect_unlink_records(
        &mut records,
        &personal,
        project_id,
        UnlinkRemoteScope::Personal,
        &["entries", "tasks", "knowledge_pages"],
    )?;

    if let Some(team_id) = team_id {
        let team_scope = UnlinkRemoteScope::Team(team_id.to_string());
        let team = syncer
            .pull_raw(project_id, &entity_types, Some(team_id))
            .map_err(|error| anyhow::anyhow!("team cloud pull failed: {error}"))?;
        collect_unlink_records(&mut records, &team, project_id, team_scope, &entity_types)?;
    }

    Ok(records.into_iter().collect())
}

fn collect_unlink_records(
    records: &mut BTreeSet<UnlinkRemoteRecord>,
    body: &serde_json::Value,
    project_id: &str,
    scope: UnlinkRemoteScope,
    keys: &[&str],
) -> anyhow::Result<()> {
    for key in keys {
        let rows = body
            .get(*key)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                anyhow::anyhow!("scoped cloud pull omitted required `{key}` array; refusing unlink")
            })?;
        for row in rows {
            let row_project_id = row
                .get("project_canonical_id")
                .or_else(|| row.get("project_id"))
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "scoped cloud pull returned `{key}` row without project_id; refusing unlink"
                    )
                })?;
            if !project_ids_match(row_project_id, project_id) {
                anyhow::bail!(
                    "scoped cloud pull returned `{key}` row for foreign project `{row_project_id}`; refusing unlink"
                );
            }
            let id = row
                .get("id")
                .or_else(|| row.get("entity_id"))
                .and_then(serde_json::Value::as_str)
                .filter(|id| !id.is_empty())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "scoped cloud pull returned `{key}` row without id; refusing unlink"
                    )
                })?;
            let entity_type = match *key {
                "entries" => "entry",
                "tasks" => "task",
                "knowledge_pages" => "knowledge_page",
                _ => unreachable!("unlink only collects explicitly supported entity types"),
            };
            records.insert(UnlinkRemoteRecord {
                scope: scope.clone(),
                entity_type: entity_type.to_string(),
                id: id.to_string(),
            });
        }
    }
    Ok(())
}

fn delete_unlink_remote_records(
    endpoint: &str,
    token: &str,
    project_id: &str,
    records: &[UnlinkRemoteRecord],
) -> anyhow::Result<usize> {
    let mut deleted = 0;
    for record in records {
        let (path, scope_label) = match &record.scope {
            UnlinkRemoteScope::Personal => (
                format!(
                    "{endpoint}/api/sync/{}/{}?project_id={}",
                    record.entity_type,
                    urlencoding::encode(&record.id),
                    urlencoding::encode(project_id)
                ),
                "personal",
            ),
            UnlinkRemoteScope::Team(team_id) => (
                format!(
                    "{endpoint}/api/teams/{}/sync/{}/{}?project_id={}",
                    urlencoding::encode(team_id),
                    record.entity_type,
                    urlencoding::encode(&record.id),
                    urlencoding::encode(project_id)
                ),
                "team",
            ),
        };
        match ureq::delete(&path)
            .timeout(Duration::from_secs(30))
            .set("Authorization", &format!("Bearer {token}"))
            .call()
        {
            Ok(response) if (200..300).contains(&response.status()) => deleted += 1,
            Err(ureq::Error::Status(404, _)) => {
                // Already absent is the desired final state for an idempotent
                // unlink and matches the existing sync delete semantics.
                deleted += 1;
            }
            Ok(response) => {
                let status = response.status();
                let body = response.into_string().unwrap_or_default();
                anyhow::bail!(
                    "{scope_label} delete of {} {} failed with status {status}: {body}; local cloud link preserved",
                    record.entity_type,
                    record.id
                )
            }
            Err(ureq::Error::Status(status, response)) => {
                let body = response.into_string().unwrap_or_default();
                anyhow::bail!(
                    "{scope_label} delete of {} {} failed with status {status}: {body}; local cloud link preserved",
                    record.entity_type,
                    record.id
                )
            }
            Err(ureq::Error::Transport(error)) => anyhow::bail!(
                "{scope_label} delete of {} {} failed: {error}; local cloud link preserved",
                record.entity_type,
                record.id
            ),
        }
    }
    Ok(deleted)
}

fn render_unlink_result(
    cli: &Cli,
    cas_root: &Path,
    project_id: Option<&str>,
    args: &CloudUnlinkArgs,
    discovered: usize,
    deleted: usize,
    local_unlinked: bool,
    detail: &str,
) -> anyhow::Result<()> {
    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "status": if args.dry_run {
                    "dry_run"
                } else if local_unlinked {
                    "ok"
                } else {
                    "skipped"
                },
                "root": cas_root,
                "project_canonical_id": project_id,
                "purge_remote": args.purge_remote,
                "dry_run": args.dry_run,
                "remote_records_discovered": discovered,
                "remote_records_deleted": deleted,
                "local_unlinked": local_unlinked,
                "detail": detail,
            })
        );
        return Ok(());
    }
    let mut out = io::stdout();
    let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
    if local_unlinked {
        fmt.success("Cloud unlink complete")?;
    } else if detail == "dry run" || detail.starts_with("dry run") {
        fmt.write_accent("Cloud unlink dry run")?;
    } else {
        fmt.success("Cloud unlink skipped")?;
    }
    fmt.newline()?;
    if let Some(project_id) = project_id {
        fmt.write_raw(&format!("    project: {project_id}"))?;
        fmt.newline()?;
    }
    if args.purge_remote {
        fmt.write_raw(&format!("    remote records discovered: {discovered}"))?;
        fmt.newline()?;
        fmt.write_raw(&format!("    remote records deleted: {deleted}"))?;
        fmt.newline()?;
    }
    fmt.write_raw(&format!("    {detail}"))?;
    fmt.newline()?;
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// TEAM — set / show / clear the active team
// ═══════════════════════════════════════════════════════════════════════════════

/// HTTP timeout for the pre-flight team-membership probe.
///
/// Same magnitude as the coordinator's default — long enough to absorb a cold
/// Neon/Vercel cache, short enough that a misconfigured endpoint fails
/// visibly instead of hanging the shell.
const TEAM_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Validate a string is a canonical UUID (36 chars, 8-4-4-4-12 hex).
///
/// Both upper- and lower-case hex are accepted; the value is normalised to
/// lowercase on return. Non-canonical UUID forms (braces `{...}`,
/// no-hyphen 32-char, URN `urn:uuid:...`) that the `uuid` crate would
/// otherwise accept are explicitly rejected — the server stores team UUIDs
/// in canonical form, and silently accepting variants would let two strings
/// represent the same team locally while compare-unequal elsewhere.
///
/// Returns a short error (no "find your UUID here" guidance); the CLI
/// layer wraps that with endpoint-specific help via `format_uuid_error`.
fn parse_team_uuid(input: &str) -> Result<String, String> {
    // Canonical hyphenated form is exactly 36 chars; reject braces,
    // URN-prefixed, and no-hyphen 32-char variants before uuid::try_parse
    // normalises them silently.
    if input.len() != 36 {
        return Err(format!("expected a team UUID, got `{input}`"));
    }
    match uuid::Uuid::try_parse(input) {
        Ok(u) => Ok(u.as_hyphenated().to_string()),
        Err(_) => Err(format!("expected a team UUID, got `{input}`")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum TeamSetTarget {
    Uuid(String),
    CachedTeam {
        query: Option<String>,
        team: TeamInfo,
    },
}

/// Result of applying the zero-argument `cas cloud team set` resolution to a
/// project immediately after login refreshed the user's membership cache.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum LoginTeamSelection {
    Activated(TeamInfo),
    NoMembership,
    MultipleMemberships,
}

impl TeamSetTarget {
    fn uuid(&self) -> &str {
        match self {
            TeamSetTarget::Uuid(uuid) => uuid,
            TeamSetTarget::CachedTeam { team, .. } => &team.id,
        }
    }

    fn slug(&self) -> Option<&str> {
        match self {
            TeamSetTarget::Uuid(_) => None,
            TeamSetTarget::CachedTeam { team, .. } => Some(&team.slug),
        }
    }

    fn name(&self) -> Option<&str> {
        match self {
            TeamSetTarget::Uuid(_) => None,
            TeamSetTarget::CachedTeam { team, .. } => Some(&team.name),
        }
    }
}

fn cached_team_options(config: &CloudConfig) -> String {
    if config.teams.is_empty() {
        "No cached teams found.".to_string()
    } else {
        let available = config
            .teams
            .iter()
            .map(|t| format!("{} ({})", t.slug, t.name))
            .collect::<Vec<_>>()
            .join(", ");
        format!("Available cached teams: {available}.")
    }
}

fn resolve_team_set_target(
    arg: Option<&str>,
    user_config: &CloudConfig,
    allow_uuid_passthrough: bool,
) -> anyhow::Result<TeamSetTarget> {
    if let Some(input) = arg {
        if allow_uuid_passthrough && let Ok(uuid) = parse_team_uuid(input) {
            return Ok(TeamSetTarget::Uuid(uuid));
        }

        if let Some(team) = user_config
            .teams
            .iter()
            .find(|t| t.slug == input || t.id == input)
        {
            return Ok(TeamSetTarget::CachedTeam {
                query: Some(input.to_string()),
                team: team.clone(),
            });
        }

        anyhow::bail!(
            "Team slug {:?} not found in cached memberships.\n{}\nRun `cas cloud login` to refresh team membership.",
            input,
            cached_team_options(user_config)
        );
    }

    match user_config.teams.as_slice() {
        [team] => Ok(TeamSetTarget::CachedTeam {
            query: None,
            team: team.clone(),
        }),
        [] => anyhow::bail!(
            "No cached team memberships found.\nRun `cas cloud login` to refresh team membership."
        ),
        _ => anyhow::bail!(
            "Multiple cached teams found; pass one explicitly.\n{}\nRun `cas cloud login` to refresh team membership.",
            cached_team_options(user_config)
        ),
    }
}

/// Activate the sole cached team for the current project.
///
/// This deliberately delegates the one/zero/many decision to the resolver
/// behind `cas cloud team set` (cas-8850), so login and the explicit command
/// cannot drift into separate membership-resolution rules. The caller persists
/// `project_config` only after an [`LoginTeamSelection::Activated`] result.
pub(crate) fn select_cached_team_after_login(
    project_config: &mut CloudConfig,
    user_config: &CloudConfig,
) -> LoginTeamSelection {
    match resolve_team_set_target(None, user_config, false) {
        Ok(TeamSetTarget::CachedTeam { team, .. }) => {
            project_config.team_id = Some(team.id.clone());
            project_config.team_slug = Some(team.slug.clone());
            LoginTeamSelection::Activated(team)
        }
        Ok(TeamSetTarget::Uuid(_)) => unreachable!("zero-argument resolution never yields UUID"),
        Err(_) if user_config.teams.is_empty() => LoginTeamSelection::NoMembership,
        Err(_) => LoginTeamSelection::MultipleMemberships,
    }
}

/// Render the post-login team-selection result.
///
/// Zero and multiple memberships retain the existing manual team-set path;
/// only a sole cached membership becomes the active project team.
pub(crate) fn print_login_team_selection(cli: &Cli, outcome: &LoginTeamSelection) {
    match outcome {
        LoginTeamSelection::Activated(team) if cli.json => eprintln!(
            "{}",
            serde_json::json!({
                "event": "login_team_activated",
                "team_id": team.id,
                "team_slug": team.slug,
                "team_name": team.name,
            })
        ),
        LoginTeamSelection::Activated(team) => {
            eprintln!();
            eprintln!("  ✓ Active team set from your only membership");
            eprintln!("    Team: {} ({})", team.name, team.slug);
            eprintln!("    UUID: {}", team.id);
        }
        LoginTeamSelection::NoMembership if cli.json => eprintln!(
            "{}",
            serde_json::json!({
                "event": "login_team_selection_required",
                "reason": "no_memberships",
                "hint": "cas cloud team set <uuid>",
            })
        ),
        LoginTeamSelection::MultipleMemberships if cli.json => eprintln!(
            "{}",
            serde_json::json!({
                "event": "login_team_selection_required",
                "reason": "multiple_memberships",
                "hint": "cas cloud team set <uuid>",
            })
        ),
        LoginTeamSelection::NoMembership => {
            eprintln!(
                "  No team membership found. Run `cas cloud team set <uuid>` to set the active team."
            );
        }
        LoginTeamSelection::MultipleMemberships => {
            eprintln!(
                "  Multiple team memberships found. Run `cas cloud team set <uuid>` to set the active team."
            );
        }
    }
}

fn load_user_cloud_config_for_team_resolution() -> anyhow::Result<CloudConfig> {
    let path = user_level_cloud_json_path().ok_or_else(|| {
        anyhow::anyhow!(
            "Cannot determine home directory. Run `cas cloud login` to refresh team membership."
        )
    })?;
    Ok(CloudConfig::load_from(&path)?)
}

/// Result of probing team membership via `GET /api/teams/{uuid}/projects`.
#[derive(Debug, PartialEq, Eq)]
enum TeamProbeOutcome {
    /// Server returned 2xx — user is a member of the team.
    Member,
    /// Server returned 401 — the token is invalid or expired.
    Unauthorized,
    /// Server returned 403 — valid token, but user is not a team member.
    NotAMember,
    /// Server returned 404 — team UUID does not exist.
    NotFound,
    /// Network error or unexpected status code.
    Error(String),
}

/// Probe team membership by hitting `GET /api/teams/{uuid}/projects`.
///
/// This endpoint already enforces `validateTeamMembership` server-side and is
/// cheap (no body), so it is the natural pre-flight check before persisting
/// `team_id` to cloud.json. Factored out for testability with wiremock.
fn probe_team_membership(endpoint: &str, token: &str, team_uuid: &str) -> TeamProbeOutcome {
    let url = format!("{endpoint}/api/teams/{team_uuid}/projects");
    match ureq::get(&url)
        .timeout(TEAM_PROBE_TIMEOUT)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
    {
        Ok(_) => TeamProbeOutcome::Member,
        Err(ureq::Error::Status(401, _)) => TeamProbeOutcome::Unauthorized,
        Err(ureq::Error::Status(403, _)) => TeamProbeOutcome::NotAMember,
        Err(ureq::Error::Status(404, _)) => TeamProbeOutcome::NotFound,
        Err(ureq::Error::Status(code, _)) => {
            TeamProbeOutcome::Error(format!("unexpected HTTP {code}"))
        }
        Err(e) => TeamProbeOutcome::Error(format!("network error: {e}")),
    }
}

/// Dispatcher for `cas cloud team` subcommands.
///
/// `pub` + `#[doc(hidden)]` so `cas-cli/tests/team_set_slug_resolution_test.rs`
/// can exercise the slug-resolution wiring against a wiremock server (matches
/// the same pattern used by `execute_sync` / `execute_team_pull`).
#[doc(hidden)]
pub fn execute_team(cmd: &CloudTeamCommands, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    match cmd {
        CloudTeamCommands::Default(args) => {
            // `default` writes to the user-level ~/.cas/cloud.json, not the
            // project's .cas/. Resolve the user cas dir here so the inner
            // function stays injected (and testable).
            let user_cas_dir = dirs::home_dir()
                .map(|h| h.join(".cas"))
                .ok_or_else(|| anyhow::anyhow!("Cannot determine home directory"))?;
            execute_team_default_inner(args, cli, &user_cas_dir)
        }
        CloudTeamCommands::Set(args) => execute_team_set(args, cli, cas_root),
        CloudTeamCommands::Auto(cmd) => execute_team_auto(cmd, cli, cas_root),
        CloudTeamCommands::Show => execute_team_show(cli, cas_root).map(|_| ()),
        CloudTeamCommands::Clear => execute_team_clear(cli, cas_root),
    }
}

/// Dispatcher for `cas cloud project` subcommands (cas-1ced).
///
/// Manual override path for `[project] canonical_id` in `.cas/config.toml`.
/// `pub` + `#[doc(hidden)]` for the same integration-test reason as
/// `execute_team`.
#[doc(hidden)]
pub fn execute_project(
    cmd: &CloudProjectCommands,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    match cmd {
        CloudProjectCommands::Set(args) => execute_project_set(args, cli, cas_root),
        CloudProjectCommands::AdoptAliases => execute_project_adopt_aliases(cli, cas_root),
    }
}

/// Write `[project] canonical_id = "<value>"` to `<cas_root>/config.toml`
/// and confirm. Used by `cas cloud project set` when auto-derivation in
/// `cas cloud team set` cannot resolve a slug (monorepo / custom layout /
/// non-git directory).
fn execute_project_set(
    args: &CloudProjectSetArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    crate::cloud::set_canonical_id_in_config_toml(cas_root, &args.canonical_id)?;
    if cli.json {
        let out = serde_json::json!({
            "status": "ok",
            "canonical_id": args.canonical_id,
        });
        println!("{}", out);
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let success_color = fmt.theme().palette.status_success;
        fmt.newline()?;
        fmt.write_colored("  \u{2713} ", success_color)?;
        fmt.write_raw("Project canonical id set")?;
        fmt.newline()?;
        fmt.write_muted("  canonical_id: ")?;
        fmt.write_raw(&args.canonical_id)?;
        fmt.newline()?;
    }
    Ok(())
}

/// Rewrite task rows whose persisted `origin_project` is a remote/case alias
/// of the current project. The change is local and queue-backed: every updated
/// task receives both a personal upsert and, when team scope is active, the
/// same team upsert that an ordinary task edit would produce.
fn execute_project_adopt_aliases(cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    use crate::store::share_policy::eligible_for_team_task;

    let project_id = crate::cloud::resolve_canonical_id(cas_root)
        .ok_or_else(|| anyhow::anyhow!("Cannot determine the current project identity"))?;
    let task_store = crate::store::open_task_store_local(cas_root)?;
    let queue = SyncQueue::open(cas_root)?;
    queue.init()?;
    let cloud_config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root).ok();
    let team_id = cloud_config.as_ref().and_then(CloudConfig::active_team_id);

    let mut aliases = BTreeMap::<String, usize>::new();
    let mut rewritten = 0usize;
    let mut enqueued = 0usize;
    for mut task in task_store.list(None)? {
        let Some(origin) = task.origin_project.as_deref() else {
            continue;
        };
        let Some(canonical_origin) =
            crate::cloud::canonical_project_id_with_pin(origin, Some(&project_id))
        else {
            continue;
        };
        if canonical_origin != project_id || origin.trim() == project_id {
            continue;
        }

        *aliases.entry(origin.to_string()).or_default() += 1;
        task.origin_project = Some(project_id.clone());
        task_store.update(&task)?;
        let persisted = task_store.get(&task.id)?;
        let payload = serde_json::to_string(&persisted)?;
        queue.enqueue(
            crate::cloud::EntityType::Task,
            &persisted.id,
            crate::cloud::SyncOperation::Upsert,
            Some(&payload),
        )?;
        if let Some(team_id) = team_id.as_deref()
            && eligible_for_team_task(&persisted)
        {
            queue.enqueue_for_team(
                crate::cloud::EntityType::Task,
                &persisted.id,
                crate::cloud::SyncOperation::Upsert,
                Some(&payload),
                team_id,
            )?;
        }
        rewritten += 1;
        enqueued += 1;
    }

    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "status": "ok",
                "project_id": project_id,
                "aliases": aliases,
                "rewritten": rewritten,
                "enqueued_upserts": enqueued,
            })
        );
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.newline()?;
        fmt.write_raw(&format!(
            "  Adopted {rewritten} alias row(s) for project {project_id}; enqueued {enqueued} upsert(s)."
        ))?;
        fmt.newline()?;
        for (alias, count) in aliases {
            fmt.write_raw(&format!("  {count} row(s) rewritten from alias {alias}"))?;
            fmt.newline()?;
        }
    }
    Ok(())
}

// ─── TEAM DEFAULT ────────────────────────────────────────────────────────────

/// Testable entrypoint for `cas cloud team default` — call this directly from
/// integration tests to avoid touching the real `~/.cas/cloud.json`.
///
/// `user_cas_dir` is normally `~/.cas/` (resolved by the dispatcher) but can
/// be any tempdir in tests — same injected-path pattern as `execute_team_set`.
#[doc(hidden)]
pub fn execute_team_default_for_test(
    args: &CloudTeamDefaultArgs,
    cli: &Cli,
    user_cas_dir: &PathBuf,
) -> anyhow::Result<serde_json::Value> {
    execute_team_default_inner(args, cli, user_cas_dir)?;
    // Return the updated config as JSON for assertion convenience.
    let cfg = CloudConfig::load_from_cas_dir(user_cas_dir)?;
    Ok(serde_json::json!({
        "default_team_id": cfg.default_team_id,
    }))
}

/// Inner implementation for `cas cloud team default`.
///
/// Accepts an injected `user_cas_dir` so integration tests can point it at a
/// tempdir.  The dispatcher resolves `~/.cas/` before calling this.
fn execute_team_default_inner(
    args: &CloudTeamDefaultArgs,
    cli: &Cli,
    user_cas_dir: &Path,
) -> anyhow::Result<()> {
    let mut config = CloudConfig::load_from_cas_dir(user_cas_dir)?;

    if args.personal {
        let was_set = config.default_team_id.is_some();
        config.default_team_id = None;
        // Mark the one-time backfill gate so future syncs do not re-promote
        // the user to team scope against their explicit personal-scope choice.
        config.team_backfill_notified = true;
        config.save_to_cas_dir(user_cas_dir)?;

        if cli.json {
            println!(
                "{}",
                serde_json::json!({ "status": "ok", "default_team_id": serde_json::Value::Null, "was_set": was_set })
            );
        } else {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            let success_color = fmt.theme().palette.status_success;
            fmt.newline()?;
            fmt.write_colored("  \u{2713} ", success_color)?;
            fmt.write_raw(if was_set {
                "Default team cleared — syncing to personal scope"
            } else {
                "No default team was configured"
            })?;
            fmt.newline()?;
        }
        return Ok(());
    }

    let query = args
        .slug_or_uuid
        .as_deref()
        .expect("slug_or_uuid is required unless --personal is set");

    match resolve_team_set_target(Some(query), &config, false)? {
        TeamSetTarget::Uuid(_) => unreachable!("team default does not allow UUID passthrough"),
        TeamSetTarget::CachedTeam { team, .. } => {
            config.default_team_id = Some(team.id.clone());
            config.save_to_cas_dir(user_cas_dir)?;

            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "status": "ok",
                        "default_team_id": team.id,
                        "team_slug": team.slug,
                        "team_name": team.name,
                    })
                );
            } else {
                let theme = ActiveTheme::default();
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme);
                let success_color = fmt.theme().palette.status_success;
                fmt.newline()?;
                fmt.write_colored("  \u{2713} ", success_color)?;
                fmt.write_raw("Default team set")?;
                fmt.newline()?;
                fmt.write_muted("  Team:  ")?;
                fmt.write_raw(&team.name)?;
                fmt.newline()?;
                fmt.write_muted("  Slug:  ")?;
                fmt.write_raw(&team.slug)?;
                fmt.newline()?;
                fmt.write_muted("  UUID:  ")?;
                fmt.write_raw(&team.id)?;
                fmt.newline()?;
            }
            Ok(())
        }
    }
}

fn execute_team_set(args: &CloudTeamSetArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    // Load config before parsing so the error path can build an
    // endpoint-aware dashboard URL ("find your team UUID at …").
    // `load_from_cas_dir` (rather than `load()`) so the test harness can
    // point team_set at a tempdir via the same `cas_root` it threads
    // through the rest of the cloud-cmd dispatcher.
    let mut config = CloudConfig::load_from_cas_dir(cas_root)?;

    let target = match args.id.as_deref() {
        Some(input) if parse_team_uuid(input).is_ok() => {
            TeamSetTarget::Uuid(parse_team_uuid(input).expect("checked above"))
        }
        other => {
            let user_config = load_user_cloud_config_for_team_resolution()?;
            resolve_team_set_target(other, &user_config, true)?
        }
    };
    let uuid = target.uuid().to_string();

    let token = config
        .token
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not logged in. Run 'cas login' first."))?
        .clone();

    match probe_team_membership(&config.endpoint, &token, &uuid) {
        TeamProbeOutcome::Member => {
            // UUID input has no local slug to persist; slug/zero-arg inputs
            // resolve through cached memberships and carry the slug forward.
            config.team_id = Some(uuid.clone());
            config.team_slug = target.slug().map(ToString::to_string);
            config.save_to_cas_dir(cas_root)?;

            // cas-1ced: eagerly resolve project canonical_id so the first
            // sync from a new clone doesn't go out with the wrong scope.
            // Resolution order: existing config.toml → git remote →
            // defer (do NOT default to the working-directory basename,
            // that's the exact bug this task fixes).
            let slug = resolve_project_slug_for_team_set(cas_root);

            // cas-f07a (AC1): warn when the resolved slug is an under-populated
            // bucket while another team project holds significantly more data.
            // Best-effort — a network failure silently skips the check; the
            // team_id is already persisted so the overall command still succeeds.
            let resolved_id_opt = match &slug {
                SlugResolution::FromConfig(s) | SlugResolution::FromGitRemote(s) => {
                    Some(s.as_str())
                }
                SlugResolution::NotResolved => None,
            };
            if let Some(resolved_id) = resolved_id_opt {
                if let Some(projects) = fetch_team_projects(&config.endpoint, &token, &uuid) {
                    if let Some((richer_id, resolved_count, richer_count)) =
                        check_bucket_ambiguity(resolved_id, &projects)
                    {
                        eprintln!(
                            "\nWarning: project bucket '{resolved_id}' has only \
                             {resolved_count} items. Team project '{richer_id}' has \
                             {richer_count} — you may be connected to the wrong \
                             bucket. Run `cas cloud project set {richer_id}` to \
                             pin the correct slug.\n"
                        );
                    }
                }
            }

            if cli.json {
                let (resolved, source) = match &slug {
                    SlugResolution::FromConfig(s) => (Some(s.as_str()), "config_toml"),
                    SlugResolution::FromGitRemote(s) => (Some(s.as_str()), "git_remote"),
                    SlugResolution::NotResolved => (None, "deferred"),
                };
                let out = serde_json::json!({
                    "status": "ok",
                    "team_id": uuid,
                    "team_slug": target.slug(),
                    "team_name": target.name(),
                    "canonical_id": resolved,
                    "canonical_id_source": source,
                });
                println!("{}", out);
            } else {
                let theme = ActiveTheme::default();
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme);
                let success_color = fmt.theme().palette.status_success;
                fmt.newline()?;
                fmt.write_colored("  \u{2713} ", success_color)?;
                fmt.write_raw("Active team set")?;
                fmt.newline()?;
                if let TeamSetTarget::CachedTeam { query, team } = &target {
                    fmt.write_muted("  Resolved: ")?;
                    match query {
                        Some(q) => {
                            fmt.write_raw("'")?;
                            fmt.write_raw(q)?;
                            fmt.write_raw("' \u{2192} ")?;
                        }
                        None => {
                            fmt.write_raw("single cached team '")?;
                            fmt.write_raw(&team.slug)?;
                            fmt.write_raw("' \u{2192} ")?;
                        }
                    }
                    fmt.write_raw(&team.id)?;
                    fmt.write_raw(" (")?;
                    fmt.write_raw(&team.name)?;
                    fmt.write_raw(")")?;
                    fmt.newline()?;
                }
                fmt.write_muted("  UUID: ")?;
                fmt.write_raw(&uuid)?;
                fmt.newline()?;
                match &slug {
                    SlugResolution::FromConfig(s) => {
                        fmt.write_muted("  Project slug: ")?;
                        fmt.write_raw(s)?;
                        fmt.write_muted(" (from .cas/config.toml)")?;
                        fmt.newline()?;
                    }
                    SlugResolution::FromGitRemote(s) => {
                        fmt.write_muted("  Project slug: ")?;
                        fmt.write_raw(s)?;
                        fmt.write_muted(" (derived from git remote)")?;
                        fmt.newline()?;
                    }
                    SlugResolution::NotResolved => {
                        fmt.write_muted(
                            "  Slug resolution deferred — run `cas cloud project set <canonical-id>`",
                        )?;
                        fmt.newline()?;
                    }
                }
            }
            Ok(())
        }
        TeamProbeOutcome::Unauthorized => {
            anyhow::bail!("Token invalid or expired. Run 'cas login' to re-authenticate.")
        }
        TeamProbeOutcome::NotAMember => {
            anyhow::bail!("You are not a member of team {uuid}.")
        }
        TeamProbeOutcome::NotFound => {
            anyhow::bail!("Team {uuid} not found on {}.", config.endpoint)
        }
        TeamProbeOutcome::Error(msg) => {
            anyhow::bail!("Failed to verify team membership: {msg}")
        }
    }
}

fn load_user_cloud_config_or_default() -> CloudConfig {
    user_level_cloud_json_path()
        .and_then(|p| CloudConfig::load_from(&p).ok())
        .unwrap_or_default()
}

fn find_team_display<'a>(
    team_id: &str,
    project: &'a CloudConfig,
    user: &'a CloudConfig,
) -> (Option<&'a str>, Option<&'a str>) {
    if project.team_id.as_deref() == Some(team_id) {
        if let Some(slug) = project.team_slug.as_deref() {
            return (Some(slug), None);
        }
    }
    if let Some(team) = user.teams.iter().find(|t| t.id == team_id) {
        return (Some(team.slug.as_str()), Some(team.name.as_str()));
    }
    (None, None)
}

fn execute_team_auto(
    cmd: &CloudTeamAutoCommands,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    let mut config = CloudConfig::load_from_cas_dir(cas_root)?;
    match cmd {
        CloudTeamAutoCommands::On => config.team_auto_promote = Some(true),
        CloudTeamAutoCommands::Off => config.team_auto_promote = Some(false),
        CloudTeamAutoCommands::Clear => config.team_auto_promote = None,
    }
    config.save_to_cas_dir(cas_root)?;

    let user_config = load_user_cloud_config_or_default();
    let effective_team = config.active_team_id_with_user_config(Some(&user_config));

    if cli.json {
        let (team_slug, team_name) = effective_team
            .as_deref()
            .map(|id| find_team_display(id, &config, &user_config))
            .unwrap_or((None, None));
        println!(
            "{}",
            serde_json::json!({
                "status": "ok",
                "team_auto_promote": config.team_auto_promote,
                "effective_team_id": effective_team,
                "effective_team_slug": team_slug,
                "effective_team_name": team_name,
            })
        );
        return Ok(());
    }

    let theme = ActiveTheme::default();
    let mut out = io::stdout();
    let mut fmt = Formatter::stdout(&mut out, theme);
    let success_color = fmt.theme().palette.status_success;
    let warning_color = fmt.theme().palette.status_warning;
    fmt.newline()?;
    fmt.write_colored("  \u{2713} ", success_color)?;
    match cmd {
        CloudTeamAutoCommands::On => fmt.write_raw("Team auto-promotion enabled")?,
        CloudTeamAutoCommands::Off => fmt.write_raw("Team auto-promotion disabled")?,
        CloudTeamAutoCommands::Clear => fmt.write_raw("Team auto-promotion override cleared")?,
    }
    fmt.newline()?;

    match effective_team.as_deref() {
        Some(team_id) => {
            let (team_slug, team_name) = find_team_display(team_id, &config, &user_config);
            fmt.write_muted("  Effective team: ")?;
            match (team_slug, team_name) {
                (Some(slug), Some(name)) => {
                    fmt.write_raw(name)?;
                    fmt.write_raw(" (")?;
                    fmt.write_raw(slug)?;
                    fmt.write_raw(")")?;
                }
                (Some(slug), None) => fmt.write_raw(slug)?,
                _ => fmt.write_raw(team_id)?,
            }
            fmt.newline()?;
            fmt.write_muted("  UUID: ")?;
            fmt.write_raw(team_id)?;
            fmt.newline()?;
        }
        None => {
            fmt.write_colored("  \u{26A0} ", warning_color)?;
            if matches!(cmd, CloudTeamAutoCommands::On) {
                fmt.write_raw(
                    "No effective team resolved. Set a user default with `cas cloud team default <slug>` or refresh memberships with `cas cloud login`.",
                )?;
            } else {
                fmt.write_raw("Effective scope: personal")?;
            }
            fmt.newline()?;
        }
    }

    Ok(())
}

fn print_personal_scope_notice(cli: &Cli, notice: &PersonalScopeNotice) {
    if cli.json {
        eprintln!(
            "{}",
            serde_json::json!({
                "event": "personal_scope_team_available",
                "team_id": notice.team_id,
                "team_slug": notice.team_slug,
                "team_name": notice.team_name,
            })
        );
    } else {
        eprintln!("{}", notice.message());
    }
}

/// Report the automatic team-scope adoption (cas-c117).
///
/// Adoption promotes this project's memories, tasks, rules and skills from
/// personal to team scope, so it is never silent: the user is told which team
/// was adopted and how to undo it. The quiet outcomes (already scoped, opted
/// out, not logged in) print nothing — routine syncs keep their output.
fn print_team_scope_adoption(cli: &Cli, adoption: &crate::cloud::TeamScopeAdoption) {
    use crate::cloud::TeamScopeAdoption;

    match adoption {
        TeamScopeAdoption::Adopted(team) => {
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "team_scope_adoption": {
                            "status": "adopted",
                            "team_id": team.team_id,
                            "team_slug": team.team_slug,
                            "team_name": team.team_name,
                        }
                    })
                );
            } else {
                eprintln!(
                    "  \u{2713} Team scope enabled for this project: {} ({})\n    \
                     This project's memories, tasks and skills now sync to your team. \
                     Run `cas cloud team auto off` to keep it personal.",
                    team.team_name, team.team_slug
                );
            }
        }
        TeamScopeAdoption::NoResolvableTeam { membership_count } if *membership_count > 1 => {
            // Genuinely ambiguous: Cassy will not guess which of several teams a
            // project belongs to, and silence here would reproduce exactly the
            // "why is nothing shared?" confusion this task exists to remove.
            if cli.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "team_scope_adoption": {
                            "status": "ambiguous",
                            "membership_count": membership_count,
                        }
                    })
                );
            } else {
                eprintln!(
                    "  \u{25CF} This project is personal: you belong to {membership_count} teams \
                     and no default is set.\n    Run `cas cloud team default <slug>` to pick one, \
                     or `cas cloud team auto off` to silence this."
                );
            }
        }
        other => {
            tracing::debug!(outcome = ?other, "team scope adoption made no change");
        }
    }
}

/// Outcome of the eager slug-resolution flow run by `cas cloud team set`
/// (cas-1ced).
///
/// Three states, in priority order:
///  - `FromConfig` — `.cas/config.toml [project] canonical_id` already
///    held a value. We leave it alone (source of truth).
///  - `FromGitRemote` — derived from `git remote get-url origin`. The
///    helper writes it to `.cas/config.toml` so subsequent resolves are
///    `FromConfig`.
///  - `NotResolved` — neither yielded a value. The handler keeps the
///    "deferred" message and explicitly does NOT default to the
///    working-directory basename (that's the bug this task fixes).
#[doc(hidden)]
pub enum SlugResolution {
    FromConfig(String),
    FromGitRemote(String),
    NotResolved,
}

/// Run the slug-resolution flow against `cas_root`. Reads config.toml
/// first; falls back to git-remote derivation; persists the derived value
/// to config.toml on success. Best-effort write: a write failure
/// downgrades the result to `NotResolved` rather than failing the team
/// set as a whole (the team_id was already persisted by the caller).
fn resolve_project_slug_for_team_set(cas_root: &Path) -> SlugResolution {
    if let Some(slug) = crate::cloud::canonical_id_from_config_toml(cas_root) {
        return SlugResolution::FromConfig(slug);
    }
    if let Some(slug) = crate::cloud::derive_canonical_id_from_git_remote(cas_root)
        .and_then(|remote| crate::cloud::canonical_project_id(&remote))
    {
        if crate::cloud::set_canonical_id_in_config_toml(cas_root, &slug).is_ok() {
            return SlugResolution::FromGitRemote(slug);
        }
    }
    SlugResolution::NotResolved
}

/// Fetch the team's project list from `/api/teams/{uuid}/projects`. Best-effort:
/// returns `None` on any network or parse failure so the caller can skip the
/// ambiguity check without failing the overall operation.
///
/// Note: `probe_team_membership` already hits this endpoint to validate
/// membership — this is a second call that reads the body. The endpoint is
/// cheap (read-only, small payload) and `cas cloud team set` is a rare
/// interactive command, so two round-trips are acceptable.
fn fetch_team_projects(
    endpoint: &str,
    token: &str,
    team_uuid: &str,
) -> Option<Vec<crate::cloud::TeamProject>> {
    let url = format!("{endpoint}/api/teams/{team_uuid}/projects");
    ureq::get(&url)
        .timeout(TEAM_PROBE_TIMEOUT)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
        .ok()?
        .into_json::<crate::cloud::TeamProjectsResponse>()
        .ok()
        .map(|r| r.projects)
}

/// Check whether the resolved `canonical_id` is an under-populated bucket
/// compared to the richest other project in the team. Returns
/// `Some((richer_id, resolved_count, richer_count))` when:
///   - `resolved_id` appears in `projects`, AND
///   - there is at least one OTHER project with ≥ 50 memories, AND
///   - the resolved project holds < 10 % of the richest other project's count.
///
/// The 10 % / 50-memory thresholds are conservative so the warning only fires
/// when the evidence strongly suggests the wrong bucket was chosen (e.g. 666
/// memories vs 19 285). New projects (0 memories) that genuinely haven't
/// synced yet will trigger the warning when another bucket is large — callers
/// should frame the warning as advisory, not conclusive.
///
/// Pure function (no IO) — tested independently in the unit-test module.
pub(crate) fn check_bucket_ambiguity(
    resolved_id: &str,
    projects: &[crate::cloud::TeamProject],
) -> Option<(String, u32, u32)> {
    let resolved = projects
        .iter()
        .find(|p| project_ids_match(&p.canonical_id, resolved_id))?;

    let richest = projects
        .iter()
        .filter(|p| !project_ids_match(&p.canonical_id, resolved_id))
        .max_by_key(|p| p.memory_count)?;

    if richest.memory_count >= 50
        && (resolved.memory_count as u64) * 10 < richest.memory_count as u64
    {
        Some((
            richest.canonical_id.clone(),
            resolved.memory_count,
            richest.memory_count,
        ))
    } else {
        None
    }
}

/// Pure-data variant of `execute_team_show` — builds the JSON payload
/// without doing IO. Used by `execute_team_show` and exposed (via the
/// `_for_test` wrapper) so integration tests can assert on the shape
/// without capturing stdout. cas-1ced.
///
/// cas-f07a (AC2): uses `resolve_canonical_id` (full 3-step chain) so the
/// output is never `null` for an active project — the folder name is the
/// minimum returned when no explicit pin exists.
fn team_show_json(cas_root: &Path) -> anyhow::Result<serde_json::Value> {
    let config = CloudConfig::load_from_cas_dir(cas_root)?;
    let canonical_id = crate::cloud::resolve_canonical_id(cas_root);
    let team = resolve_team_display_for_show(&config);
    Ok(serde_json::json!({
        "team_id": team.team_id,
        "team_slug": team.team_slug,
        "team_name": team.team_name,
        "canonical_id": canonical_id,
    }))
}

/// The team identity `cas cloud team show` displays, resolved exactly the
/// way `cas cloud team auto` resolves it (cas-c117, field-report finding #6).
struct ResolvedTeamDisplay {
    team_id: Option<String>,
    team_slug: Option<String>,
    team_name: Option<String>,
}

/// Resolve the displayed team identity from the project config first and the
/// user-level membership cache second.
///
/// Before this, `team show` printed only `config.team_slug`, which
/// `cas cloud team set <uuid>` leaves as `None` (a raw UUID carries no slug —
/// see `execute_team_set`). The result was `Team slug: <not resolved>`
/// immediately after `cas cloud team auto on` had printed
/// "Effective team: Petra Stella (petra-stella)" for the very same UUID,
/// because `team auto` resolves through [`find_team_display`] and the
/// user-level `teams[]` cache. Both surfaces now share that resolution.
///
/// The project-level `team_id` still wins over the user-level effective team
/// so an explicit `cas cloud team set` is never hidden by the auto-promotion
/// kill switch.
fn resolve_team_display_for_show(config: &CloudConfig) -> ResolvedTeamDisplay {
    let user_config = load_user_cloud_config_or_default();
    let team_id = config
        .team_id
        .clone()
        .or_else(|| config.active_team_id_with_user_config(Some(&user_config)));

    match team_id {
        Some(id) => {
            let (slug, name) = find_team_display(&id, config, &user_config);
            ResolvedTeamDisplay {
                team_slug: slug.map(ToString::to_string),
                team_name: name.map(ToString::to_string),
                team_id: Some(id),
            }
        }
        None => ResolvedTeamDisplay {
            team_id: None,
            team_slug: None,
            team_name: None,
        },
    }
}

/// Test-only entrypoint that returns the rendered JSON string. `pub` so
/// `cas-cli/tests/team_set_slug_resolution_test.rs` can assert on the
/// composed output (team UUID + resolved project slug) without capturing
/// stdout. `#[doc(hidden)]` so it doesn't pollute the public API surface.
#[doc(hidden)]
pub fn execute_team_show_for_test(_cli: &Cli, cas_root: &Path) -> anyhow::Result<String> {
    let value = team_show_json(cas_root)?;
    Ok(value.to_string())
}

fn execute_team_show(cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let config = CloudConfig::load_from_cas_dir(cas_root)?;
    // cas-c117 finding #6: resolve the slug the same way `team auto` does —
    // project config first, user-level membership cache second — so the two
    // commands can never disagree about the same UUID.
    let resolved = resolve_team_display_for_show(&config);

    match (&resolved.team_id, &resolved.team_slug) {
        (Some(id), slug) => {
            // cas-f07a (AC2): use the full resolution chain so the slug is
            // never shown as "<not resolved>" for an active project.  The
            // 3-step chain is: config.toml → folder name → path hash.  Only
            // the config-toml step was called before this fix; the folder-name
            // fallback makes the output actionable even without an explicit
            // pin.
            let canonical_id = crate::cloud::resolve_canonical_id(cas_root);
            if cli.json {
                let out = serde_json::json!({
                    "team_id": id,
                    "team_slug": slug,
                    "team_name": resolved.team_name,
                    "canonical_id": canonical_id,
                });
                println!("{}", out);
            } else {
                let theme = ActiveTheme::default();
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme);
                fmt.newline()?;
                fmt.write_muted("  Team ID:      ")?;
                fmt.write_raw(id)?;
                fmt.newline()?;
                fmt.write_muted("  Team slug:    ")?;
                match (slug.as_deref(), resolved.team_name.as_deref()) {
                    (Some(slug), Some(name)) => {
                        fmt.write_raw(slug)?;
                        fmt.write_raw(" (")?;
                        fmt.write_raw(name)?;
                        fmt.write_raw(")")?;
                    }
                    (Some(slug), None) => fmt.write_raw(slug)?,
                    // Unresolved now means the membership cache is stale, not
                    // that the team is unknown — say so instead of leaving a
                    // bare sentinel the user cannot act on.
                    (None, _) => fmt.write_raw(
                        "<not resolved — run `cas cloud login` to refresh team memberships>",
                    )?,
                }
                fmt.newline()?;
                fmt.write_muted("  Project slug: ")?;
                fmt.write_raw(canonical_id.as_deref().unwrap_or("<not resolved>"))?;
                fmt.newline()?;
            }
        }
        (None, _) => {
            if cli.json {
                let out = serde_json::json!({
                    "team_id": serde_json::Value::Null,
                });
                println!("{}", out);
            } else {
                let theme = ActiveTheme::default();
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme);
                let warning_color = fmt.theme().palette.status_warning;
                fmt.newline()?;
                fmt.write_colored("  \u{25CF} ", warning_color)?;
                fmt.write_raw("No team configured")?;
                fmt.newline()?;
                fmt.write_raw("  Run ")?;
                fmt.write_accent("cas cloud team set <uuid>")?;
                fmt.write_raw(" to set the active team.")?;
                fmt.newline()?;
            }
        }
    }
    Ok(())
}

fn execute_team_clear(cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let mut config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)?;
    let was_set = config.team_id.is_some();
    config.clear_team();
    config.save_to_cas_dir(cas_root)?;

    if cli.json {
        let out = serde_json::json!({ "status": "ok", "was_set": was_set });
        println!("{}", out);
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let success_color = fmt.theme().palette.status_success;
        fmt.newline()?;
        fmt.write_colored("  \u{2713} ", success_color)?;
        fmt.write_raw(if was_set {
            "Active team cleared"
        } else {
            "No team was configured"
        })?;
        fmt.newline()?;
        // cas-c117: sync now adopts a resolvable team automatically, so
        // clearing alone is not "make this project personal" — say so rather
        // than letting the next sync silently re-adopt.
        fmt.write_muted(
            "  The next sync re-adopts your default team. Run `cas cloud team auto off` to keep this project personal.",
        )?;
        fmt.newline()?;
    }
    Ok(())
}

fn execute_conflicts(args: &CloudConflictsArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let queue = SyncQueue::open(cas_root)?;
    queue.init()?;
    if let Some(days) = args.prune {
        let pruned = queue.prune_conflicts(days)?;
        if cli.json {
            println!("{}", serde_json::json!({"status": "ok", "pruned": pruned}));
        } else {
            println!("Pruned {pruned} retained conflicts older than {days} days");
        }
        return Ok(());
    }

    let conflicts = queue.list_conflicts(args.limit)?;
    let count = queue.unreviewed_conflict_count()?;
    if cli.json {
        println!(
            "{}",
            serde_json::json!({"count": count, "conflicts": conflicts})
        );
    } else if conflicts.is_empty() {
        println!("No retained cloud sync conflicts.");
    } else {
        println!("{count} retained cloud sync conflict(s):");
        for conflict in conflicts {
            println!(
                "{} {} — winner: {}, strategy: {}, resolved: {}",
                conflict.entity_type,
                conflict.entity_id,
                conflict.winner_side,
                conflict.strategy,
                conflict.resolved_at
            );
        }
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// LOGIN - Polished TUI with Device Flow
// ═══════════════════════════════════════════════════════════════════════════════

/// Local distilled-knowledge counts for `cas cloud status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct KnowledgeCounts {
    pub pages: usize,
    pub pending_embedding: usize,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct KnowledgeSyncSummary {
    pushed: usize,
    pulled: usize,
    embedded: usize,
}

/// Count local knowledge pages, or `None` when this project has no knowledge
/// store yet. Never an error path: a status command must not fail because a
/// repo has not been distilled.
pub(crate) fn local_knowledge_counts(cas_root: &Path) -> Option<KnowledgeCounts> {
    use cas_store::{KnowledgeStore, SqliteKnowledgeStore};

    let store = SqliteKnowledgeStore::open(cas_root).ok()?;
    let pages = store.list_pages().ok()?;
    if pages.is_empty() {
        return None;
    }
    let pending_embedding = pages.iter().filter(|p| p.pending_embedding).count();
    Some(KnowledgeCounts {
        pages: pages.len(),
        pending_embedding,
    })
}

/// Push, pull and embed distilled knowledge as part of `cas cloud sync`.
///
/// Entirely optional: without cloud auth this returns immediately, having made
/// no network call and created no vector storage. Failures are reported and
/// swallowed — knowledge distribution is an enhancement, and it must never
/// take down a sync that already moved entries and tasks.
fn sync_project_knowledge_with_output(
    cli: &Cli,
    cas_root: &Path,
    emit_output: bool,
) -> anyhow::Result<KnowledgeSyncSummary> {
    use crate::cloud::embeddings::{
        DEFAULT_EMBED_BATCH, KnowledgeEmbedder, KnowledgeVectorCache, embed_pending_pages,
    };
    use crate::cloud::{CloudSyncer, CloudSyncerConfig, resolve_canonical_id_for_sync};
    use cas_store::{KnowledgeStore, SqliteKnowledgeStore};
    use std::sync::Arc;

    let config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)?;
    if !config.is_logged_in() {
        return Ok(KnowledgeSyncSummary::default());
    }

    let store = match SqliteKnowledgeStore::open(cas_root) {
        Ok(store) => store,
        Err(e) => {
            tracing::debug!(error = %e, "knowledge sync skipped: no knowledge store");
            return Ok(KnowledgeSyncSummary::default());
        }
    };

    let queue = Arc::new(SyncQueue::open(cas_root)?);
    queue.init()?;
    let project_id = resolve_canonical_id_for_sync(cas_root)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let syncer = CloudSyncer::new_for_project(
        queue,
        config.clone(),
        CloudSyncerConfig::default(),
        project_id,
        cas_root,
    );

    let pushed = match syncer.push_knowledge_pages(&store) {
        Ok(count) => count,
        Err(e) => {
            tracing::warn!(error = %e, "knowledge page push failed (non-fatal)");
            0
        }
    };
    let pulled = match syncer.pull_knowledge_pages(&store) {
        Ok(report) => {
            for (rel_path, message) in &report.errors {
                tracing::warn!(page = %rel_path, error = %message, "knowledge page pull error");
            }
            report
        }
        Err(e) => {
            tracing::warn!(error = %e, "knowledge page pull failed (non-fatal)");
            Default::default()
        }
    };

    // Embeddings: only when the capability is present. `from_config` returning
    // None is the first gate — no auth, no embedder, no vector cache on disk,
    // no cloud call. Whether the endpoint actually implements the capability is
    // discovered on first use and reported as `capability_absent`.
    let mut embedded = 0usize;
    // Everything the run could not cover, so the summary can say so out loud
    // instead of printing a cheerful "0 embedded" (cas-a924).
    let mut awaiting = store.count_pending_embedding().unwrap_or(0);
    let mut embed_requests = 0usize;
    let mut embed_problems: Vec<String> = Vec::new();
    let mut embed_capability_absent = false;

    if let Some(embedder) = KnowledgeEmbedder::from_config(&config) {
        match KnowledgeVectorCache::open(cas_root, embedder.meta()) {
            Ok(cache) => {
                match embed_pending_pages(&store, &embedder, &cache, DEFAULT_EMBED_BATCH) {
                    Ok(report) => {
                        embedded = report.embedded;
                        awaiting = report.pending_after;
                        embed_requests = report.requests;
                        embed_capability_absent = report.capability_absent;
                        if report.reindexed {
                            tracing::info!("embedding model changed: knowledge vectors re-indexed");
                        }
                        embed_problems.extend(report.request_errors.iter().cloned());
                        if report.rejected_zero > 0 {
                            embed_problems.push(format!(
                                "{} page(s) got an unusable zero vector",
                                report.rejected_zero
                            ));
                        }
                        if report.rejected_dims > 0 {
                            embed_problems.push(format!(
                                "{} page(s) got the wrong vector dimension",
                                report.rejected_dims
                            ));
                        }
                        for (id, message) in &report.errors {
                            embed_problems.push(format!("{id}: {message}"));
                        }
                    }
                    Err(e) => embed_problems.push(e.to_string()),
                }
            }
            Err(e) => embed_problems.push(format!("could not open knowledge vector cache: {e}")),
        }
    } else if awaiting > 0 {
        embed_problems.push("no cloud embedding capability configured (not logged in)".to_string());
    }

    if emit_output && cli.json {
        println!(
            "{}",
            serde_json::json!({
                "knowledge": {
                    "pushed": pushed,
                    "pulled": pulled.applied,
                    "locked_preserved": pulled.locked_preserved,
                    "tombstones_applied": pulled.tombstones_applied,
                    "tombstones_locked_preserved": pulled.tombstones_locked_preserved,
                    "tombstoned_pages_refused": pulled.tombstoned_pages_refused,
                    "refused_foreign": pulled.refused_foreign,
                    "refused_foreign_ids": pulled.refused_foreign_ids,
                    "starvation_warning": pulled.starvation_warning,
                    "embedded": embedded,
                    "embed_requests": embed_requests,
                    "awaiting_embedding": awaiting,
                    "embedding_capability_absent": embed_capability_absent,
                    "embed_problems": embed_problems,
                }
            })
        );
    } else if emit_output {
        if pushed > 0 || pulled.applied > 0 || embedded > 0 {
            println!(
                "  Knowledge: {pushed} pushed, {} pulled, {embedded} embedded",
                pulled.applied
            );
        }
        // The failure mode with no error attached — say it out loud or it is
        // indistinguishable from a quiet, healthy project.
        if let Some(warning) = &pulled.starvation_warning {
            eprintln!("  Knowledge: POSSIBLE SYNC STARVATION — {warning}");
        }
        // A refusal is never a silent drop: it is a contamination attempt the
        // operator needs to see, named, with the ids involved.
        if pulled.refused_foreign > 0 {
            eprintln!(
                "  Knowledge: REFUSED {} foreign page(s) at ingest: {}",
                pulled.refused_foreign,
                pulled.refused_foreign_ids.join(", ")
            );
        }
        if pulled.tombstones_locked_preserved > 0 {
            eprintln!(
                "  Knowledge: preserved {} locked page(s) against incoming tombstone(s)",
                pulled.tombstones_locked_preserved
            );
        }
        if pulled.tombstoned_pages_refused > 0 {
            eprintln!(
                "  Knowledge: refused {} stale page record(s) after a tombstone",
                pulled.tombstoned_pages_refused
            );
        }
        // The loud half: never let a failed or partial embed pass as silence.
        if awaiting > 0 {
            println!("  Knowledge: {awaiting} page(s) still awaiting an embedding");
        }
        if embed_capability_absent {
            println!(
                "  Knowledge: this cloud endpoint does not provide embeddings — semantic search stays local-only"
            );
        }
        for problem in &embed_problems {
            eprintln!("  Knowledge: embedding problem: {problem}");
        }
    }

    Ok(KnowledgeSyncSummary {
        pushed,
        pulled: pulled.applied,
        embedded,
    })
}

fn execute_status(cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)?;

    if config.token.is_none() {
        if cli.json {
            println!(r#"{{"status":"not_logged_in"}}"#);
        } else {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            let warning_color = fmt.theme().palette.status_warning;
            fmt.write_colored("  \u{25CF} ", warning_color)?;
            fmt.write_raw("Not logged in to Cassy Cloud")?;
            fmt.newline()?;
            fmt.write_raw("  Run ")?;
            fmt.write_accent("cas login")?;
            fmt.write_raw(" to authenticate")?;
            fmt.newline()?;
        }
        return Ok(());
    }

    {
        let status_url = format!("{}/api/sync/status", config.endpoint);
        let token = config.token.as_ref().unwrap();

        match ureq::get(&status_url)
            .set("Authorization", &format!("Bearer {token}"))
            .call()
        {
            Ok(resp) => {
                let body: serde_json::Value = resp.into_json()?;

                if cli.json {
                    let mut body = body.clone();
                    if let (Some(obj), Some(counts)) =
                        (body.as_object_mut(), local_knowledge_counts(cas_root))
                    {
                        obj.insert(
                            "local_knowledge_pages".to_string(),
                            serde_json::json!(counts.pages),
                        );
                        obj.insert(
                            "local_knowledge_pages_pending_embedding".to_string(),
                            serde_json::json!(counts.pending_embedding),
                        );
                    }
                    println!("{}", serde_json::to_string(&body)?);
                } else {
                    let theme = ActiveTheme::default();
                    let mut out = io::stdout();
                    let mut fmt = Formatter::stdout(&mut out, theme);
                    let success_color = fmt.theme().palette.status_success;
                    let warning_color = fmt.theme().palette.status_warning;

                    fmt.newline()?;
                    fmt.write_colored("  \u{25CF} ", success_color)?;
                    fmt.write_raw("Cassy Cloud")?;
                    fmt.newline()?;
                    fmt.newline()?;

                    if let Some(email) = &config.email {
                        fmt.write_muted("  Email:  ")?;
                        fmt.write_raw(email)?;
                        fmt.newline()?;
                    }
                    fmt.write_muted("  Server: ")?;
                    fmt.write_raw(&config.endpoint)?;
                    fmt.newline()?;

                    let active_team = config.active_team_id();
                    fmt.write_muted("  Team:   ")?;
                    if let Some(team_id) = active_team.as_deref() {
                        let slug = config
                            .teams
                            .iter()
                            .find(|team| team.id == team_id)
                            .map(|team| team.slug.as_str())
                            .or(config.team_slug.as_deref())
                            .unwrap_or("configured-team");
                        fmt.write_raw(&format!("{slug} ({team_id})"))?;
                    } else {
                        fmt.write_raw("personal only (no team configured)")?;
                    }
                    fmt.newline()?;
                    let daemon_live = open_agent_store(cas_root)
                        .ok()
                        .and_then(|store| store.is_daemon_active(60).ok())
                        .unwrap_or(false);
                    fmt.write_muted("  Embedded daemon/cas serve: ")?;
                    fmt.write_raw(if daemon_live {
                        "running"
                    } else {
                        "not running"
                    })?;
                    fmt.newline()?;

                    if let Some(state) = body.get("sync_state") {
                        fmt.newline()?;
                        fmt.write_muted("  Entries: ")?;
                        fmt.write_raw(
                            &state
                                .get("entry_count")
                                .unwrap_or(&serde_json::json!(0))
                                .to_string(),
                        )?;
                        fmt.newline()?;
                        fmt.write_muted("  Tasks:  ")?;
                        fmt.write_raw(
                            &state
                                .get("task_count")
                                .unwrap_or(&serde_json::json!(0))
                                .to_string(),
                        )?;
                        fmt.newline()?;
                    }

                    // Local distilled knowledge (T5). Counted locally on
                    // purpose: the local store is the source of truth for
                    // project knowledge, so this line stays truthful even
                    // when the server has never seen a page.
                    if let Some(counts) = local_knowledge_counts(cas_root) {
                        fmt.write_muted("  Knowledge: ")?;
                        fmt.write_raw(&format!("{} pages", counts.pages))?;
                        if counts.pending_embedding > 0 {
                            fmt.write_muted(&format!(
                                " ({} awaiting embedding)",
                                counts.pending_embedding
                            ))?;
                        }
                        fmt.newline()?;
                    }

                    // Show local queue stats
                    if let Ok(queue) = crate::cloud::SyncQueue::open(cas_root) {
                        if queue.init().is_ok() {
                            if let Ok(stats) = queue.stats(5) {
                                fmt.write_muted("  Queue:  ")?;
                                fmt.write_raw(&format!(
                                    "{} pending, {} failed",
                                    stats.pending, stats.failed
                                ))?;
                                fmt.newline()?;
                                fmt.write_muted("  Last successful pull: ")?;
                                fmt.write_raw(
                                    &queue
                                        .get_metadata("last_pull_at")?
                                        .unwrap_or_else(|| "never".to_string()),
                                )?;
                                fmt.newline()?;
                                if stats.total > 0 {
                                    fmt.newline()?;
                                    fmt.write_colored("  \u{25CF} ", warning_color)?;
                                    fmt.write_raw("Sync Queue")?;
                                    fmt.newline()?;
                                    fmt.write_raw(&format!(
                                        "    {} pending, {} failed",
                                        stats.pending, stats.failed
                                    ))?;
                                    fmt.newline()?;
                                    fmt.write_raw("    Run ")?;
                                    fmt.write_accent("cas cloud queue")?;
                                    fmt.write_raw(" for details")?;
                                    fmt.newline()?;
                                }
                            }
                        }
                    }
                    fmt.newline()?;
                }
            }
            Err(ureq::Error::Status(401, _)) => {
                if cli.json {
                    println!(r#"{{"status":"error","message":"Invalid token"}}"#);
                } else {
                    let theme = ActiveTheme::default();
                    let mut err = io::stderr();
                    let mut fmt = Formatter::stdout(&mut err, theme);
                    let error_color = fmt.theme().palette.status_error;
                    fmt.write_colored("  \u{2717} ", error_color)?;
                    fmt.write_raw("Session expired")?;
                    fmt.newline()?;
                    fmt.write_raw("  Run ")?;
                    fmt.write_accent("cas login")?;
                    fmt.write_raw(" to re-authenticate")?;
                    fmt.newline()?;
                }
            }
            Err(e) => {
                anyhow::bail!("Failed to connect: {e}");
            }
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// QUEUE - View and manage sync queue
// ═══════════════════════════════════════════════════════════════════════════════

fn execute_queue(args: &CloudQueueArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    use crate::cloud::SyncQueue;

    let queue = SyncQueue::open(cas_root)?;
    queue.init()?;

    // Handle clear operation
    if args.clear {
        queue.clear()?;
        if cli.json {
            println!(r#"{{"status":"ok","message":"Queue cleared"}}"#);
        } else {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.success("Queue cleared")?;
        }
        return Ok(());
    }

    // Handle prune operation
    if let Some(days) = args.prune {
        let max_retries = 5; // Default max retries
        let pruned = queue.prune_failed(days, max_retries)?;
        if cli.json {
            println!(r#"{{"status":"ok","pruned":{pruned}}}"#);
        } else {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.success(&format!(
                "Pruned {} failed items older than {} days",
                pruned, days
            ))?;
        }
        return Ok(());
    }

    // Reset only terminal failures. Their diagnostics remain on the rows so
    // operators retain the server evidence while a repaired cloud endpoint
    // gets another chance to accept them.
    if args.retry {
        let max_retries = 5;
        let retried = match args.retry_reason.as_deref() {
            Some(reason) => queue.retry_failed_for_reason(reason, max_retries)?,
            None => queue.retry_failed(max_retries)?,
        };
        if cli.json {
            let mut output = serde_json::json!({
                "status": "ok",
                "retried": retried,
            });
            if let Some(reason) = &args.retry_reason {
                output["reason"] = serde_json::Value::String(reason.clone());
            }
            println!("{output}");
        } else {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            let message = match &args.retry_reason {
                Some(reason) => {
                    format!("Requeued {retried} failed item(s) matching reason {reason:?}")
                }
                None => format!("Requeued {retried} failed item(s)"),
            };
            fmt.success(&message)?;
        }
        return Ok(());
    }

    // Show queue stats
    let max_retries = 5;
    let stats = queue.stats(max_retries)?;

    if cli.json {
        if args.verbose {
            let items = queue.list_all(args.limit)?;
            println!(
                "{}",
                serde_json::json!({
                    "stats": stats,
                    "items": items
                })
            );
        } else {
            println!("{}", serde_json::to_string(&stats)?);
        }
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);

        if stats.total == 0 {
            let success_color = fmt.theme().palette.status_success;
            fmt.write_colored("  \u{25CF} ", success_color)?;
            fmt.write_raw("Sync queue is empty")?;
            fmt.newline()?;
            return Ok(());
        }

        let accent_color = fmt.theme().palette.accent;
        let error_color = fmt.theme().palette.status_error;
        let warning_color = fmt.theme().palette.status_warning;

        fmt.newline()?;
        fmt.write_colored("  \u{25CF} ", accent_color)?;
        fmt.write_raw("Sync Queue")?;
        fmt.newline()?;
        fmt.newline()?;
        fmt.write_muted("  Total:   ")?;
        fmt.write_raw(&stats.total.to_string())?;
        fmt.newline()?;
        fmt.write_muted("  Pending: ")?;
        fmt.write_raw(&stats.pending.to_string())?;
        fmt.newline()?;
        fmt.write_muted("  Failed:  ")?;
        fmt.write_raw(&stats.failed.to_string())?;
        fmt.newline()?;

        if !stats.by_type.is_empty() {
            fmt.newline()?;
            fmt.write_muted("  By type:")?;
            fmt.newline()?;
            for (entity_type, count) in &stats.by_type {
                fmt.write_raw(&format!("    {entity_type}: {count}"))?;
                fmt.newline()?;
            }
        }

        if let Some(oldest) = &stats.oldest_item {
            fmt.newline()?;
            fmt.write_muted("  Oldest: ")?;
            fmt.write_raw(oldest)?;
            fmt.newline()?;
        }

        // Show detailed list if verbose
        if args.verbose {
            let items = queue.list_all(args.limit)?;
            if !items.is_empty() {
                fmt.newline()?;
                fmt.write_muted("  Queued items:")?;
                fmt.newline()?;
                for item in items {
                    fmt.write_raw("    ")?;
                    if item.retry_count >= max_retries {
                        fmt.write_colored("\u{2717}", error_color)?;
                    } else if item.retry_count > 0 {
                        fmt.write_colored("\u{21BB}", warning_color)?;
                    } else {
                        fmt.write_muted("\u{25CB}")?;
                    }
                    fmt.write_raw(&format!(
                        " {} {} ({})",
                        item.operation.as_str(),
                        item.entity_id,
                        item.entity_type.as_str()
                    ))?;
                    fmt.newline()?;

                    if item.retry_count > 0 {
                        fmt.write_muted("      ")?;
                        fmt.write_raw(&format!(" retries: {}", item.retry_count))?;
                        fmt.newline()?;
                    }
                    if let Some(err) = &item.last_error {
                        fmt.write_muted("      ")?;
                        fmt.write_raw(&format!(" error: {}", err))?;
                        fmt.newline()?;
                    }
                }
            }
        }
        fmt.newline()?;
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// PUSH
// ═══════════════════════════════════════════════════════════════════════════════

/// Rehome guard for `cas cloud push` (AC6 — defect D).
///
/// Verifies that the `project_canonical_id` used for the next push matches the
/// one recorded from the last successful push. A changed slug would cause the
/// cloud server to re-home ALL existing entities into the new project bucket —
/// a surprising and difficult-to-reverse side effect of a normal push.
///
/// Returns `Ok(())` when the push is safe to proceed:
/// - No prior push recorded (first push for this project).
/// - The slug is unchanged since the last push.
/// - `rehome` is `true` (user explicitly confirmed the re-home with `--rehome`).
///
/// Returns `Err(message)` with a user-facing explanation when a slug change is
/// detected and `rehome` is `false`.
///
/// `pub` + `#[doc(hidden)]` so integration tests in `cas-cli/tests/` can
/// exercise the guard without a live HTTP server — same convention as
/// `execute_team_push` / `execute_team_pull`.
#[doc(hidden)]
pub fn check_canonical_id_rehome(
    queue: &SyncQueue,
    project_id: &str,
    rehome: bool,
) -> Result<(), String> {
    // Fail-open: if the metadata read errors (malformed DB, etc.) we let the
    // push proceed. The guard will re-run on the next invocation once the DB
    // is healthy. The metadata is only authoritative when it exists.
    let stored = queue.get_metadata("last_push_canonical_id").unwrap_or(None);
    match stored {
        None => Ok(()), // First push — no prior slug on record
        Some(ref stored_id)
            if crate::cloud::canonical_project_id(stored_id)
                == crate::cloud::canonical_project_id(project_id) =>
        {
            Ok(()) // Case/protocol drift only — unchanged, safe.
        }
        Some(stored_id) => {
            if rehome {
                Ok(()) // User explicitly confirmed the re-home with --rehome
            } else {
                Err(format!(
                    "push refused: project slug changed from `{stored_id}` to `{project_id}`.\n\
                     Pushing with a different slug would re-home all existing cloud entities\n\
                     into the new project bucket — a potentially large, hard-to-reverse operation.\n\
                     \n\
                     To confirm the re-home, pass --rehome:\n\
                       cas cloud push --rehome\n\
                     \n\
                     To restore the previous slug (no re-home):\n\
                       cas cloud project set {stored_id}"
                ))
            }
        }
    }
}

fn push_result_counts(
    result: &crate::cloud::SyncResult,
    scope: crate::cloud::PushScope,
) -> std::collections::BTreeMap<&'static str, usize> {
    let all = [
        ("entries", result.pushed_entries),
        ("tasks", result.pushed_tasks),
        ("rules", result.pushed_rules),
        ("skills", result.pushed_skills),
        ("sessions", result.pushed_sessions),
        ("verifications", result.pushed_verifications),
        ("events", result.pushed_events),
        ("prompts", result.pushed_prompts),
        ("file_changes", result.pushed_file_changes),
        ("commit_links", result.pushed_commit_links),
        ("agents", result.pushed_agents),
        ("worktrees", result.pushed_worktrees),
    ];
    all.into_iter()
        .filter(|(key, count)| {
            *count > 0
                || matches!(scope, crate::cloud::PushScope::EntriesOnly if *key == "entries")
                || matches!(scope, crate::cloud::PushScope::TasksOnly if *key == "tasks")
        })
        .collect()
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct SkippedTeamBacklog {
    team_id: String,
    pending: usize,
    failed: usize,
    command: &'static str,
}

impl SkippedTeamBacklog {
    fn total(&self) -> usize {
        self.pending + self.failed
    }
}

/// The display-ready result of one cloud push or pull operation.
///
/// Keeping the presentation data separate from [`crate::cloud::SyncResult`]
/// lets command handlers preserve their JSON wire shape while `cas update`
/// and the interactive cloud commands share one human renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSummary {
    pub kind: SyncSummaryKind,
    pub counts: BTreeMap<String, usize>,
    /// Counts produced by the team-side operation, kept separate from the
    /// current project's personal counts for a useful combined receipt.
    pub team_counts: BTreeMap<String, usize>,
    pub batches: usize,
    pub pending: usize,
    pub failed: usize,
    pub failures: Vec<String>,
    pub errors: Vec<String>,
    pub team_backlog_pending: usize,
    pub team_backlog_failed: usize,
    pub team_configured: bool,
    pub task_transition: Option<String>,
    pub knowledge_pushed: usize,
    pub knowledge_pulled: usize,
    pub knowledge_embedded: usize,
    pub conflicts_resolved: usize,
    pub conflicts_resolved_local: usize,
    pub conflicts_resolved_remote: usize,
    pub conflicts: Vec<crate::cloud::SyncConflict>,
    pub team_conflicts_resolved: usize,
    pub team_conflicts_resolved_local: usize,
    pub team_conflicts_resolved_remote: usize,
    pub team_conflicts: Vec<crate::cloud::SyncConflict>,
    pub healed_task_dependencies_to_cloud: usize,
    pub healed_task_dependencies_from_cloud: usize,
    pub team_healed_task_dependencies_to_cloud: usize,
    pub team_healed_task_dependencies_from_cloud: usize,
    /// Local edges removed because the cloud carried a deletion tombstone.
    pub deleted_task_dependencies: usize,
    /// Local edges a tombstone kept out of the push queue.
    pub skipped_task_dependencies_by_tombstone: usize,
    pub team_errors: Vec<String>,
    pub team_push_attention: usize,
    /// Rows the cloud kept a newer version of and the client acknowledged.
    /// Reported apart from failures: nothing is lost and nothing needs a retry.
    pub skipped_lww: usize,
    /// Terminal rows the cloud explicitly refused, grouped by its reason.
    pub rejected_by_reason: BTreeMap<String, usize>,
    /// Terminal rows this build requeued once because an older client parked them.
    pub requeued_after_upgrade: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncSummaryKind {
    Push,
    Pull,
}

impl SyncSummary {
    fn push(
        result: &crate::cloud::SyncResult,
        scope: crate::cloud::PushScope,
        team_backlog: Option<SkippedTeamBacklog>,
    ) -> Self {
        let counts = push_result_counts(result, scope)
            .into_iter()
            .map(|(key, count)| (key.to_string(), count))
            .collect();
        let failures = if result.remaining_backlog.failed > 0
            && !result.remaining_backlog.failed_errors.is_empty()
        {
            result.remaining_backlog.failed_errors.clone()
        } else {
            result.errors.clone()
        };
        Self {
            kind: SyncSummaryKind::Push,
            counts,
            team_counts: BTreeMap::new(),
            batches: result.batches_run,
            pending: result.remaining_backlog.pending,
            failed: result.remaining_backlog.failed,
            failures,
            errors: result.errors.clone(),
            team_backlog_pending: team_backlog
                .as_ref()
                .map(|backlog| backlog.pending)
                .unwrap_or_default(),
            team_backlog_failed: team_backlog
                .as_ref()
                .map(|backlog| backlog.failed)
                .unwrap_or_default(),
            team_configured: team_backlog.is_some(),
            task_transition: None,
            knowledge_pushed: result.pushed_knowledge_pages,
            knowledge_pulled: result.pulled_knowledge_pages,
            knowledge_embedded: 0,
            conflicts_resolved: result.conflicts_resolved,
            conflicts_resolved_local: result.conflicts_resolved_local,
            conflicts_resolved_remote: result.conflicts_resolved_remote,
            conflicts: result.conflicts.clone(),
            team_conflicts_resolved: 0,
            team_conflicts_resolved_local: 0,
            team_conflicts_resolved_remote: 0,
            team_conflicts: Vec::new(),
            healed_task_dependencies_to_cloud: result.healed_task_dependencies_to_cloud,
            healed_task_dependencies_from_cloud: result.healed_task_dependencies_from_cloud,
            team_healed_task_dependencies_to_cloud: 0,
            team_healed_task_dependencies_from_cloud: 0,
            deleted_task_dependencies: result.deleted_task_dependencies,
            skipped_task_dependencies_by_tombstone: result.skipped_task_dependencies_by_tombstone,
            team_errors: Vec::new(),
            team_push_attention: 0,
            skipped_lww: result.skipped_lww_acked,
            rejected_by_reason: result.remaining_backlog.rejected_by_reason.clone(),
            requeued_after_upgrade: result.requeued_after_upgrade,
        }
    }

    fn pull(result: &crate::cloud::SyncResult, team_configured: bool) -> Self {
        let counts = [
            ("entries", result.pulled_entries),
            ("tasks", result.pulled_tasks),
            ("rules", result.pulled_rules),
            ("skills", result.pulled_skills),
            ("specs", result.pulled_specs),
            ("events", result.pulled_events),
            ("prompts", result.pulled_prompts),
            ("file changes", result.pulled_file_changes),
            ("commit links", result.pulled_commit_links),
            ("task dependencies", result.pulled_task_dependencies),
        ]
        .into_iter()
        .filter(|(_, count)| *count > 0)
        .map(|(key, count)| (key.to_string(), count))
        .collect();
        Self {
            kind: SyncSummaryKind::Pull,
            counts,
            team_counts: BTreeMap::new(),
            batches: 0,
            pending: 0,
            failed: 0,
            failures: Vec::new(),
            errors: result.errors.clone(),
            team_backlog_pending: 0,
            team_backlog_failed: 0,
            team_configured,
            task_transition: task_transition_summary(result),
            knowledge_pushed: result.pushed_knowledge_pages,
            knowledge_pulled: result.pulled_knowledge_pages,
            knowledge_embedded: 0,
            conflicts_resolved: result.conflicts_resolved,
            conflicts_resolved_local: result.conflicts_resolved_local,
            conflicts_resolved_remote: result.conflicts_resolved_remote,
            conflicts: result.conflicts.clone(),
            team_conflicts_resolved: 0,
            team_conflicts_resolved_local: 0,
            team_conflicts_resolved_remote: 0,
            team_conflicts: Vec::new(),
            healed_task_dependencies_to_cloud: result.healed_task_dependencies_to_cloud,
            healed_task_dependencies_from_cloud: result.healed_task_dependencies_from_cloud,
            team_healed_task_dependencies_to_cloud: 0,
            team_healed_task_dependencies_from_cloud: 0,
            deleted_task_dependencies: result.deleted_task_dependencies,
            skipped_task_dependencies_by_tombstone: result.skipped_task_dependencies_by_tombstone,
            team_errors: Vec::new(),
            team_push_attention: 0,
            skipped_lww: 0,
            rejected_by_reason: BTreeMap::new(),
            requeued_after_upgrade: 0,
        }
    }

    pub(crate) fn is_push(&self) -> bool {
        self.kind == SyncSummaryKind::Push
    }

    pub(crate) fn is_pull(&self) -> bool {
        self.kind == SyncSummaryKind::Pull
    }

    fn with_knowledge(mut self, knowledge: KnowledgeSyncSummary) -> Self {
        self.knowledge_pushed = knowledge.pushed;
        self.knowledge_pulled = knowledge.pulled;
        self.knowledge_embedded = knowledge.embedded;
        self
    }

    fn merge_team_summary(&mut self, team: &SyncSummary) {
        self.team_counts.extend(team.counts.clone());
        self.team_conflicts_resolved += team.conflicts_resolved;
        self.team_conflicts_resolved_local += team.conflicts_resolved_local;
        self.team_conflicts_resolved_remote += team.conflicts_resolved_remote;
        self.team_conflicts.extend(team.conflicts.clone());
        self.team_healed_task_dependencies_to_cloud += team.healed_task_dependencies_to_cloud;
        self.team_healed_task_dependencies_from_cloud += team.healed_task_dependencies_from_cloud;
        self.deleted_task_dependencies += team.deleted_task_dependencies;
        self.skipped_task_dependencies_by_tombstone += team.skipped_task_dependencies_by_tombstone;
        self.team_errors.extend(team.errors.clone());
        self.skipped_lww += team.skipped_lww;
        for (reason, count) in &team.rejected_by_reason {
            *self.rejected_by_reason.entry(reason.clone()).or_default() += count;
        }
        self.requeued_after_upgrade += team.requeued_after_upgrade;
        if team.is_push() {
            self.team_push_attention += team.errors.len();
        }
        self.team_configured = true;
    }

    fn merge_team_pull(&mut self, result: &crate::cloud::SyncResult) {
        let team = Self::pull(result, true);
        self.merge_team_summary(&team);
    }

    fn push_complete(&self) -> bool {
        self.is_push()
            && self.errors.is_empty()
            && self.pending == 0
            && self.failed == 0
            && self.team_backlog_pending == 0
            && self.team_backlog_failed == 0
            && self.team_push_attention == 0
    }
}

fn pull_summary_label(key: &str, count: usize) -> String {
    let singular = match key {
        "entries" => "entry",
        "tasks" => "task",
        "rules" => "rule",
        "skills" => "skill",
        "specs" => "spec",
        "events" => "event",
        "prompts" => "prompt",
        "file changes" => "file change",
        "commit links" => "commit link",
        "task dependencies" => "task dependency",
        _ => key,
    };
    if count == 1 {
        format!("1 {singular}")
    } else {
        format!("{count} {key}")
    }
}

fn summary_count_labels(counts: &BTreeMap<String, usize>) -> Vec<String> {
    counts
        .iter()
        .map(|(key, count)| pull_summary_label(&key.replace('_', " "), *count))
        .collect()
}

fn conflict_summary_line(conflict: &crate::cloud::SyncConflict) -> String {
    format!(
        "[Cassy sync] Conflict resolved: {} {} local={} remote={} strategy={:?} action={:?}",
        conflict.entity_type,
        conflict.entity_id,
        conflict.local_updated.format("%H:%M:%S"),
        conflict.remote_updated.format("%H:%M:%S"),
        conflict.resolution,
        conflict.action,
    )
}

fn concise_push_failure(error: &str) -> String {
    let mut message = error
        .split("; server response:")
        .next()
        .unwrap_or(error)
        .trim()
        .to_string();
    if let Some(fields) = message.strip_prefix("permanent cloud rejection: ") {
        if let Some(reason) = fields
            .split("; ")
            .find_map(|field| field.strip_prefix("reason="))
        {
            return reason.to_string();
        }
    }
    if let Some((_, suffix)) = message.split_once(": Push failed with status ") {
        message = format!("Push failed with status {suffix}");
    }
    if let Some((prefix, body)) = message.split_once(": {") {
        let json = format!("{{{body}");
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&json) {
            if let Some(server_error) = value.get("error").and_then(|value| value.as_str()) {
                return server_error.to_string();
            }
        }
        if prefix.ends_with("Push failed with status 400") {
            return message;
        }
    }
    message
}

/// Render the cloud's own rejection reasons, most frequent first.
///
/// A reason is the actionable half of a refusal; a bare count is not. The
/// label stays compact for the one-line receipt and the caller decides how
/// many remediation hints to spend space on.
fn rejection_reason_labels(summary: &SyncSummary) -> Vec<(String, usize)> {
    let mut reasons = summary
        .rejected_by_reason
        .iter()
        .map(|(reason, count)| (reason.clone(), *count))
        .collect::<Vec<_>>();
    reasons.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    reasons
}

fn rejected_total(summary: &SyncSummary) -> usize {
    summary.rejected_by_reason.values().sum()
}

fn grouped_push_failures(summary: &SyncSummary, verbose: bool) -> Vec<(String, usize)> {
    let mut groups = BTreeMap::<String, usize>::new();
    for error in &summary.failures {
        *groups.entry(concise_push_failure(error)).or_default() += 1;
    }
    if groups.is_empty() && summary.failed > 0 {
        groups.insert("unknown error".to_string(), summary.failed);
    }
    let mut groups = groups.into_iter().collect::<Vec<_>>();
    if !verbose {
        groups.truncate(3);
    }
    groups
}

/// Render one human-facing cloud sync result.
pub(crate) fn render_sync_summary(
    fmt: &mut Formatter,
    summary: &SyncSummary,
    verbose: bool,
) -> io::Result<()> {
    match summary.kind {
        SyncSummaryKind::Pull => {
            let count_details = [
                "entries",
                "tasks",
                "rules",
                "skills",
                "specs",
                "events",
                "prompts",
                "file changes",
                "commit links",
                "task dependencies",
            ]
            .into_iter()
            .filter_map(|key| {
                summary
                    .counts
                    .get(key)
                    .map(|count| pull_summary_label(key, *count))
            })
            .collect::<Vec<_>>();
            let mut details = if count_details.is_empty() {
                vec!["nothing newer".to_string()]
            } else {
                vec![count_details.join(", ")]
            };
            let conflicts = summary.conflicts_resolved + summary.team_conflicts_resolved;
            if conflicts > 0 {
                details.push(format!("{conflicts} conflicts resolved"));
            }
            let team_count_details = summary_count_labels(&summary.team_counts);
            if !team_count_details.is_empty() {
                details.push(format!("team {}", team_count_details.join(", ")));
            }
            // Name what actually happened to edges. "healed" alone read as
            // churn when the same thousands re-queued on every pull (cas-cf1f).
            let pushed_edges = summary.healed_task_dependencies_to_cloud
                + summary.team_healed_task_dependencies_to_cloud;
            let pulled_edges = summary.healed_task_dependencies_from_cloud
                + summary.team_healed_task_dependencies_from_cloud;
            let mut edge_details = Vec::new();
            if pushed_edges > 0 {
                edge_details.push(format!("{pushed_edges} pushed"));
            }
            if pulled_edges > 0 {
                edge_details.push(format!("{pulled_edges} pulled"));
            }
            if summary.deleted_task_dependencies > 0 {
                edge_details.push(format!("{} deleted", summary.deleted_task_dependencies));
            }
            if summary.skipped_task_dependencies_by_tombstone > 0 {
                edge_details.push(format!(
                    "{} skipped (tombstoned)",
                    summary.skipped_task_dependencies_by_tombstone
                ));
            }
            if !edge_details.is_empty() {
                details.push(format!("edges {}", edge_details.join(", ")));
            }
            if summary.knowledge_pushed > 0
                || summary.knowledge_pulled > 0
                || summary.knowledge_embedded > 0
            {
                let mut knowledge = Vec::new();
                if summary.knowledge_pushed > 0 {
                    knowledge.push(format!("{} pushed", summary.knowledge_pushed));
                }
                if summary.knowledge_pulled > 0 {
                    knowledge.push(format!("{} pulled", summary.knowledge_pulled));
                }
                if summary.knowledge_embedded > 0 {
                    knowledge.push(format!("{} embedded", summary.knowledge_embedded));
                }
                details.push(format!("knowledge {}", knowledge.join(", ")));
            }
            if let Some(transition) = &summary.task_transition {
                if !verbose {
                    details.push(transition.clone());
                }
            }
            details.push(if summary.team_configured {
                "team + personal".to_string()
            } else {
                "personal only".to_string()
            });
            let error_count = summary.errors.len() + summary.team_errors.len();
            let message = if error_count == 0 {
                format!("Pull complete · {}", details.join(" · "))
            } else {
                format!(
                    "Pull incomplete · {} errors · {}",
                    error_count,
                    details.join(" · ")
                )
            };
            if error_count == 0 {
                fmt.success(&message)?;
            } else {
                fmt.warning(&message)?;
            }
            if verbose {
                for key in [
                    "entries",
                    "tasks",
                    "rules",
                    "skills",
                    "specs",
                    "events",
                    "prompts",
                    "file changes",
                    "commit links",
                    "task dependencies",
                ] {
                    let count = summary.counts.get(key).copied().unwrap_or_default();
                    fmt.write_raw(&format!("    {count} {key} synced"))?;
                    fmt.newline()?;
                }
                if let Some(transition) = &summary.task_transition {
                    fmt.write_raw(transition)?;
                    fmt.newline()?;
                }
                let conflict_count = summary.conflicts.len() + summary.team_conflicts.len();
                if conflict_count > 0 {
                    fmt.write_raw(&format!("    {conflict_count} conflict(s) resolved"))?;
                    fmt.newline()?;
                }
                for conflict in summary
                    .conflicts
                    .iter()
                    .chain(summary.team_conflicts.iter())
                {
                    fmt.write_raw(&format!("    {}", conflict_summary_line(conflict)))?;
                    fmt.newline()?;
                }
                for error in summary.errors.iter().chain(summary.team_errors.iter()) {
                    fmt.write_muted("    - ")?;
                    fmt.write_raw(error)?;
                    fmt.newline()?;
                }
            }
        }
        SyncSummaryKind::Push => {
            let team_pending = summary.team_backlog_pending;
            let team_failed = summary.team_backlog_failed;
            let push_complete = summary.push_complete();
            let groups = grouped_push_failures(summary, verbose);
            if !verbose {
                if push_complete {
                    let mut parts = vec![
                        format!("{} batches", summary.batches),
                        format!("{} pending", summary.pending),
                    ];
                    if summary.skipped_lww > 0 {
                        parts.push(format!("{} kept newer by cloud", summary.skipped_lww));
                    }
                    let team_counts = summary_count_labels(&summary.team_counts);
                    if !team_counts.is_empty() {
                        parts.push(format!("team {}", team_counts.join(", ")));
                    }
                    fmt.success(&format!("Push complete · {}", parts.join(" · ")))?;
                } else {
                    let reasons = rejection_reason_labels(summary);
                    let rejected = rejected_total(summary);
                    // Rejected rows are a named subset of the terminal count;
                    // reporting both totals separately keeps "failed" honest
                    // about what the cloud never explained.
                    let failed = summary
                        .failed
                        .max(groups.iter().map(|(_, count)| *count).sum::<usize>())
                        .saturating_sub(rejected);
                    let mut parts = Vec::new();
                    if summary.skipped_lww > 0 {
                        parts.push(format!("{} kept newer by cloud", summary.skipped_lww));
                    }
                    if rejected > 0 {
                        let named = reasons
                            .iter()
                            .take(3)
                            .map(|(reason, count)| format!("{reason} ×{count}"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        parts.push(format!("{rejected} rejected by cloud ({named})"));
                        if let Some((reason, _)) = reasons.first() {
                            parts.push(format!(
                                "{reason}: {}",
                                crate::cloud::push_reason_hint(reason)
                            ));
                        }
                    }
                    if failed > 0 {
                        let failures = groups
                            .iter()
                            .map(|(message, count)| {
                                if groups.len() == 1 {
                                    message.clone()
                                } else {
                                    format!("{message} ({count})")
                                }
                            })
                            .collect::<Vec<_>>()
                            .join(", ");
                        parts.push(format!("{failed} rows failed ({failures})"));
                    }
                    if summary.team_push_attention > 0 {
                        parts.push(format!(
                            "Team push needs attention: {} issue(s)",
                            summary.team_push_attention
                        ));
                    }
                    let pending = summary.pending + team_pending;
                    if pending > 0 {
                        parts.push(format!("{pending} pending"));
                    }
                    if parts.is_empty() {
                        parts.push("queued rows remain".to_string());
                    }
                    parts.push("run cas cloud queue --retry".to_string());
                    fmt.warning(&format!("Push incomplete · {}", parts.join(" · ")))?;
                }
            } else {
                if push_complete {
                    fmt.success("Push complete")?;
                } else {
                    fmt.warning("Push incomplete; queued rows remain")?;
                }
                fmt.write_raw(&format!("    batches: {}", summary.batches))?;
                fmt.newline()?;
                fmt.write_raw(&format!(
                    "    remaining: {} pending, {} failed/parked",
                    summary.pending + team_pending,
                    summary.failed + team_failed
                ))?;
                fmt.newline()?;
                if summary.skipped_lww > 0 {
                    fmt.write_raw(&format!(
                        "    kept newer by cloud (acknowledged, removed from queue): {}",
                        summary.skipped_lww
                    ))?;
                    fmt.newline()?;
                }
                if summary.requeued_after_upgrade > 0 {
                    fmt.write_raw(&format!(
                        "    requeued after client upgrade: {}",
                        summary.requeued_after_upgrade
                    ))?;
                    fmt.newline()?;
                }
                for (reason, count) in rejection_reason_labels(summary) {
                    fmt.write_raw(&format!(
                        "    rejected by cloud: {count} row(s) reason={reason} — {}",
                        crate::cloud::push_reason_hint(&reason)
                    ))?;
                    fmt.newline()?;
                }
                for (key, count) in &summary.counts {
                    fmt.write_raw(&format!("    {key}: {count} pushed"))?;
                    fmt.newline()?;
                }
                for error in &summary.failures {
                    fmt.write_raw(&format!("    remaining error: {error}"))?;
                    fmt.newline()?;
                }
                for error in &summary.errors {
                    fmt.write_raw(&format!("    error: {error}"))?;
                    fmt.newline()?;
                }
                for key in summary.team_counts.keys() {
                    if let Some(count) = summary.team_counts.get(key) {
                        fmt.write_raw(&format!("    team {key}: {count} pushed"))?;
                        fmt.newline()?;
                    }
                }
                for error in &summary.team_errors {
                    fmt.write_raw(&format!("    team error: {error}"))?;
                    fmt.newline()?;
                }
                if team_pending > 0 || team_failed > 0 {
                    fmt.write_raw(&format!(
                        "    team backlog skipped: {} row(s); run `cas cloud sync`",
                        team_pending + team_failed
                    ))?;
                    fmt.newline()?;
                }
            }
        }
    }
    Ok(())
}

/// Personal `cloud push` deliberately does not send the team endpoint. Make
/// that boundary visible whenever the active team's queue has rows, so a
/// successful personal push cannot falsely imply that the whole queue moved.
fn active_team_backlog(
    queue: &SyncQueue,
    config: &CloudConfig,
) -> anyhow::Result<Option<SkippedTeamBacklog>> {
    let Some(team_id) = config.active_team_id() else {
        return Ok(None);
    };
    let pending =
        queue.pending_count_for_team(&team_id, CloudSyncerConfig::default().max_retries)?;
    let failed = queue.failed_count_for_team(&team_id, CloudSyncerConfig::default().max_retries)?;
    if pending == 0 && failed == 0 {
        return Ok(None);
    }
    Ok(Some(SkippedTeamBacklog {
        team_id,
        pending,
        failed,
        command: "cas cloud sync",
    }))
}

/// Queue-driven personal push, rooted explicitly at `cas_root`.
#[doc(hidden)]
pub fn execute_push(
    args: &CloudPushArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<SyncSummary> {
    execute_push_with_output(args, cli, cas_root, true)
}

fn execute_push_with_output(
    args: &CloudPushArgs,
    cli: &Cli,
    cas_root: &Path,
    emit_output: bool,
) -> anyhow::Result<SyncSummary> {
    use std::sync::Arc;

    use crate::cloud::{CloudSyncer, PushScope, resolve_canonical_id_for_sync};

    let config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)?;
    if config.token.is_none() {
        anyhow::bail!("Not logged in. Run 'cas login' first");
    }
    let project_id = resolve_canonical_id_for_sync(cas_root)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let scope = if args.entries_only {
        PushScope::EntriesOnly
    } else if args.tasks_only {
        PushScope::TasksOnly
    } else {
        PushScope::All
    };

    let queue = Arc::new(SyncQueue::open(cas_root)?);
    queue.init()?;
    let syncer = CloudSyncer::new_for_project(
        queue.clone(),
        config.clone(),
        CloudSyncerConfig::default(),
        project_id.clone(),
        cas_root,
    );
    let plan = syncer.plan_push(scope)?;
    let team_backlog = active_team_backlog(&queue, &config)?;

    if args.dry_run {
        if emit_output && cli.json {
            let mut output = serde_json::json!({
                    "dry_run": true,
                    "root": cas_root,
                    "project_canonical_id": project_id,
                    "plan": plan,
                    "max_batches": args.max_batches,
            });
            if let Some(backlog) = &team_backlog {
                output["team_backlog_skipped"] = serde_json::to_value(backlog)?;
            }
            println!("{output}");
        } else if emit_output {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
            fmt.write_accent("  \u{2192} ")?;
            fmt.write_raw("Dry run - would drain all matching queued push batches:")?;
            fmt.newline()?;
            fmt.write_raw(&format!("    root: {}", cas_root.display()))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    project: {project_id}"))?;
            fmt.newline()?;
            fmt.write_raw(&format!("    scope: {}", scope.label()))?;
            fmt.newline()?;
            fmt.write_raw(&format!(
                "    matching backlog: {} row(s)",
                plan.total_matching
            ))?;
            fmt.newline()?;
            fmt.write_raw(&format!(
                "    batch limit: {} row(s) per request",
                plan.batch_limit
            ))?;
            fmt.newline()?;
            if let Some(max_batches) = args.max_batches {
                fmt.write_raw(&format!("    max batches: {max_batches}"))?;
                fmt.newline()?;
            }
            for (key, count) in &plan.counts {
                fmt.write_raw(&format!("    next batch: {count} {key}"))?;
                fmt.newline()?;
            }
            if let Some(backlog) = &team_backlog {
                fmt.write_raw(&format!(
                    "    team backlog skipped: {} row(s); run `cas cloud sync`",
                    backlog.total()
                ))?;
                fmt.newline()?;
            }
        }
        return Ok(SyncSummary::push(
            &crate::cloud::SyncResult::default(),
            scope,
            team_backlog,
        ));
    }

    if let Err(msg) = check_canonical_id_rehome(&queue, &project_id, args.rehome) {
        if emit_output && cli.json {
            println!("{}", serde_json::json!({"status": "error", "message": msg}));
        } else if emit_output {
            let mut err = io::stderr();
            let mut fmt = Formatter::stdout(&mut err, ActiveTheme::default());
            let error_color = fmt.theme().palette.status_error;
            fmt.write_colored("  \u{2717} ", error_color)?;
            fmt.write_raw(&msg)?;
            fmt.newline()?;
        }
        return Ok(SyncSummary::push(
            &crate::cloud::SyncResult::default(),
            scope,
            team_backlog,
        ));
    }

    let result = match args.max_batches {
        Some(max_batches) => syncer.push_scoped_with_max_batches(scope, max_batches)?,
        None => syncer.push_scoped(scope)?,
    };
    if result.errors.is_empty() {
        if let Err(error) = queue.set_metadata("last_push_canonical_id", &project_id) {
            tracing::warn!(%error, %project_id, "failed to record last push project scope");
        }
    }
    let counts = push_result_counts(&result, scope);
    let summary = SyncSummary::push(&result, scope, team_backlog.clone());

    if emit_output && cli.json {
        let mut output = serde_json::json!({
                "status": if summary.push_complete() { "ok" } else { "partial" },
                "source": "sync_queue",
                "root": cas_root,
                "project_canonical_id": project_id,
                "scope": scope,
                "pushed": counts,
                "total_pushed": result.total_pushed(),
                "batches_run": result.batches_run,
                "remaining_backlog": result.remaining_backlog,
                "conflicts_resolved": result.conflicts_resolved,
                "conflicts_resolved_local": result.conflicts_resolved_local,
                "conflicts_resolved_remote": result.conflicts_resolved_remote,
                "healed_task_dependencies_to_cloud": result.healed_task_dependencies_to_cloud,
                "healed_task_dependencies_from_cloud": result.healed_task_dependencies_from_cloud,
                "deleted_task_dependencies": result.deleted_task_dependencies,
                "skipped_task_dependencies_by_tombstone":
                    result.skipped_task_dependencies_by_tombstone,
                "errors": result.concise_errors(),
        });
        if let Some(backlog) = &team_backlog {
            output["team_backlog_skipped"] = serde_json::to_value(backlog)?;
        }
        println!("{output}");
    } else if emit_output {
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
        render_sync_summary(&mut fmt, &summary, cli.verbose)?;
    }

    Ok(summary)
}

// ═══════════════════════════════════════════════════════════════════════════════
// PULL
// ═══════════════════════════════════════════════════════════════════════════════

/// Render the lifecycle portion of a pull receipt. Kept separate from row
/// counts because operators need to spot a status mutation immediately, while
/// ordinary body/note updates are expected background sync traffic.
fn task_transition_summary(result: &crate::cloud::SyncResult) -> Option<String> {
    if result.task_status_transitions.is_empty() {
        return None;
    }

    let mut groups = BTreeMap::<(String, String, String, String), usize>::new();
    for transition in &result.task_status_transitions {
        *groups
            .entry((
                transition.project_id.clone(),
                transition.source.clone(),
                transition.from.to_string(),
                transition.to.to_string(),
            ))
            .or_default() += 1;
    }
    let details = groups
        .into_iter()
        .map(|((project, source, from, to), count)| {
            format!("project={project} source={source} {from}→{to} ({count})")
        })
        .collect::<Vec<_>>()
        .join("; ");
    Some(format!(
        "Task status transitions: {} task(s) — {details}",
        result.task_status_transitions.len()
    ))
}

fn execute_pull(args: &CloudPullArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<SyncSummary> {
    // The delegated implementation clears the `last_team_pull_at_` watermark
    // for `--full` and invokes `execute_team_pull` for standalone pulls too.
    execute_pull_with_output(args, cli, cas_root, true)
}

fn execute_pull_with_output(
    args: &CloudPullArgs,
    cli: &Cli,
    cas_root: &Path,
    emit_output: bool,
) -> anyhow::Result<SyncSummary> {
    use std::sync::Arc;

    use crate::cloud::{CloudSyncer, CloudSyncerConfig, SyncQueue};

    let config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)?;
    if config.token.is_none() {
        anyhow::bail!("Not logged in. Run 'cas login' first");
    }

    // Stores synced by CloudSyncer::pull. cas-ed15 collapsed the unscoped
    // inline path through the scoped syncer for entries/tasks/rules/skills;
    // cas-bba4 re-adds the remaining 5 entity kinds (specs/events/prompts/
    // file_changes/commit_links) — also scoped — so `cas cloud pull` once
    // again imports the full set without re-introducing the leak.
    //
    // cas-7fbb: use *_local openers so pull apply does not re-enqueue into
    // SyncQueue (open_store wraps Syncing* when logged in → push↔pull loop).
    let store = open_store_local(cas_root)?;
    let task_store = open_task_store_local(cas_root)?;
    let rule_store = open_rule_store_local(cas_root)?;
    let skill_store = open_skill_store_local(cas_root)?;
    let spec_store = open_spec_store(cas_root)?;
    let event_store = open_event_store(cas_root)?;
    let prompt_store = open_prompt_store(cas_root)?;
    let file_change_store = open_file_change_store(cas_root)?;
    let commit_link_store = open_commit_link_store(cas_root)?;

    {
        use crate::ui::components::{Spinner, clear_inline, render_inline_view};

        let theme = ActiveTheme::default();
        let prev_lines = if emit_output && !cli.json && io::stdout().is_terminal() {
            let spinner = Spinner::new("Pulling from cloud...");
            render_inline_view(&spinner, &theme)?
        } else {
            0u16
        };

        // Construct the scoped syncer. Same pattern as `execute_sync` /
        // `execute_purge_foreign` (cli/cloud.rs:2106). `CloudSyncer::pull`
        // hard-fails when the supplied root has no canonical identity and always
        // appends `?project_id=<urlencoded>` to `/api/sync/pull`.
        let queue = SyncQueue::open(cas_root)?;
        queue.init()?;

        // --full: clear the watermark so the syncer issues a full (no `since=`)
        // pull. This preserves the prior `--full` semantics under the new path.
        //
        // When a team is also configured, clear the team-pull watermark too
        // for the CURRENT (team_id, project_id) scope (key format
        // `last_team_pull_at_{team_id}_{project_id}`, written by
        // `CloudSyncer::pull_team` after cas-53d5). Scope-isolation is
        // intentional: clearing only the active scope leaves watermarks for
        // other projects the user has worked on with this team intact, so
        // `cas cloud pull --full` in project P1 does NOT force a full
        // backfill on the next pull from project P2. Without project scope,
        // `--full` would either be half-broken (personal cleared, team
        // kept its old watermark) or over-broad (nukes every project's
        // team-pull watermark).
        let project_id = crate::cloud::resolve_canonical_id_for_sync(cas_root)
            .map_err(|error| anyhow::anyhow!(error.to_string()))?;
        if args.full {
            queue.delete_metadata("last_pull_at")?;
            queue.delete_metadata("last_knowledge_pull_at")?;
            queue.delete_metadata("knowledge_empty_pull_streak")?;
            if let Some(team_id) = config.active_team_id() {
                queue.delete_metadata(&format!("last_team_pull_at_{team_id}_{project_id}"))?;
            }
        }

        let syncer = CloudSyncer::new_for_project(
            Arc::new(queue),
            // Clone: the outer `config` is reused after this scope to call
            // `execute_team_pull` (cas-6ec7 wire-up).
            config.clone(),
            CloudSyncerConfig::default(),
            project_id,
            cas_root,
        );

        let pull_result = syncer.pull(
            store.as_ref(),
            task_store.as_ref(),
            rule_store.as_ref(),
            skill_store.as_ref(),
            spec_store.as_ref(),
            event_store.as_ref(),
            prompt_store.as_ref(),
            file_change_store.as_ref(),
            commit_link_store.as_ref(),
        )?;

        // The `--entries-only` / `--tasks-only` flags previously gated the
        // client-side imports of those two kinds. CloudSyncer::pull does not
        // take filter arguments; preserving these as no-ops keeps the CLI
        // contract stable for callers that pass them. The flags will become
        // semantically meaningful again if syncer-level filtering is added.
        let _ = (args.entries_only, args.tasks_only);

        let entries_count = pull_result.pulled_entries;
        let tasks_count = pull_result.pulled_tasks;
        let rules_count = pull_result.pulled_rules;
        let skills_count = pull_result.pulled_skills;
        let specs_count = pull_result.pulled_specs;
        let events_count = pull_result.pulled_events;
        let prompts_count = pull_result.pulled_prompts;
        let file_changes_count = pull_result.pulled_file_changes;
        let commit_links_count = pull_result.pulled_commit_links;
        let mut summary = SyncSummary::pull(&pull_result, config.active_team_id().is_some());

        if prev_lines > 0 {
            clear_inline(prev_lines)?;
        }

        // Fold the team pull into the personal result before rendering so a
        // normal pull has one receipt rather than one personal and one team
        // line. JSON keeps its historical separate wrapper below.
        let team_output = emit_output && cli.json;
        if let Some(team_summary) =
            execute_team_pull_with_output(&config, cas_root, cli, team_output)?
        {
            summary.merge_team_summary(&team_summary);
        }

        if emit_output && cli.json {
            let mut output = serde_json::json!({
                    "status": "ok",
                    "entries": entries_count,
                    "tasks": tasks_count,
                    "rules": rules_count,
                    "skills": skills_count,
                    "specs": specs_count,
                    "events": events_count,
                    "prompts": prompts_count,
                    "file_changes": file_changes_count,
                    "commit_links": commit_links_count,
                    "conflicts_resolved": pull_result.conflicts_resolved,
                    "conflicts_resolved_local": pull_result.conflicts_resolved_local,
                    "conflicts_resolved_remote": pull_result.conflicts_resolved_remote,
                    "healed_task_dependencies_to_cloud":
                        pull_result.healed_task_dependencies_to_cloud,
                    "healed_task_dependencies_from_cloud":
                        pull_result.healed_task_dependencies_from_cloud,
                    "deleted_task_dependencies": pull_result.deleted_task_dependencies,
                    "skipped_task_dependencies_by_tombstone":
                        pull_result.skipped_task_dependencies_by_tombstone,
                    "errors": &pull_result.errors,
            });
            if !pull_result.task_status_transitions.is_empty() {
                output["task_status_transitions"] =
                    serde_json::to_value(&pull_result.task_status_transitions)?;
            }
            println!("{output}");
        } else if emit_output {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
            render_sync_summary(&mut fmt, &summary, cli.verbose)?;
        }

        // Return the display-ready result so callers such as `cas update` can
        // render the cloud column without scraping command output.
        return Ok(summary);
    }
}

// ═══════════════════════════════════════════════════════════════════════════════
// SYNC
// ═══════════════════════════════════════════════════════════════════════════════

/// Print the T6 first-run backfill notice to stderr.
///
/// Extracted as a shared helper so both `execute_sync` and the auth login paths
/// can emit the same notice without forking the display logic.
///
/// `pub(crate)` so `cli/auth.rs` can call it without going through the public
/// API surface.
pub(crate) fn print_backfill_notice(cli: &Cli, outcome: &BackfillOutcome) {
    if let BackfillOutcome::AppliedSetDefault {
        team_id,
        team_slug,
        team_name,
    } = outcome
    {
        if !cli.json {
            eprintln!();
            eprintln!("  ✓ Team membership detected — syncing to team scope");
            eprintln!("    Team: {} ({})", team_name, team_slug);
            eprintln!("    UUID: {}", team_id);
            eprintln!();
            eprintln!("  Existing personal entries are NOT automatically promoted.");
            eprintln!("  To promote them retroactively, run:");
            eprintln!("    cas memory share --all");
            eprintln!();
            eprintln!("  To revert to personal scope:");
            eprintln!("    cas cloud team default --personal");
            eprintln!();
        } else {
            // JSON callers get a structured event they can grep for.
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "team_backfill_applied",
                    "team_id": team_id,
                    "team_slug": team_slug,
                    "team_name": team_name,
                })
            );
        }
    }
}

/// Orchestrates `cas cloud sync` — personal push, team push, then personal pull
/// (which transitively does team pull when a team is configured).
///
/// `pub` so `cas-cli/tests/team_pull_wiring_test.rs` can exercise the
/// end-to-end wire-up against a wiremock server. Production callers go
/// through the CLI dispatcher; this is not intended for external public-API
/// use. Mirrors the same `pub` + `#[doc(hidden)]` pattern as
/// `execute_team_push` / `execute_team_pull`.
#[doc(hidden)]
pub fn execute_sync(args: &CloudSyncArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    execute_sync_with_output(args, cli, cas_root, true).map(|_| ())
}

/// Execute a cloud sync and return the display-ready summaries for the
/// operations it performed. The regular CLI entry point above intentionally
/// keeps its historical `Result<()>` API; `cas update` uses this seam to put
/// the same cloud result in its project table and detail section.
pub(crate) fn execute_sync_with_summaries(
    args: &CloudSyncArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<Vec<SyncSummary>> {
    execute_sync_with_output(args, cli, cas_root, false)
}

fn execute_sync_with_output(
    args: &CloudSyncArgs,
    cli: &Cli,
    cas_root: &Path,
    emit_output: bool,
) -> anyhow::Result<Vec<SyncSummary>> {
    let mut summaries = Vec::new();
    // T2 lazy refresh: re-fetch /api/me when teams[] is empty or the last
    // fetch is more than 24 h old.  Best-effort — failure is logged but does
    // not abort sync.
    if !args.dry_run {
        let user_cfg = user_level_cloud_json_path().and_then(|p| {
            p.parent()
                .map(|d| CloudConfig::load_from_cas_dir(d).ok())
                .flatten()
        });
        let stale = user_cfg
            .as_ref()
            .map(|cfg| teams_cache_stale(cfg, 86_400))
            .unwrap_or(true);

        if stale {
            // Only refresh when we have a token; load from project config
            // (that's where the token lives after login).
            if let Ok(proj_cfg) =
                CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)
            {
                if let Some(token) = proj_cfg.token.as_deref() {
                    match fetch_and_cache_teams(&proj_cfg.endpoint, token) {
                        FetchTeamsOutcome::Updated { team_count } => {
                            tracing::debug!(
                                team_count,
                                "lazy-refreshed team membership from /api/me during sync"
                            );
                        }
                        FetchTeamsOutcome::Empty => {
                            tracing::debug!("lazy /api/me refresh: zero team memberships");
                        }
                        FetchTeamsOutcome::AuthFailed if emit_output => {
                            eprintln!(
                                "warning: could not refresh team membership (/api/me 401). \
                                 Token may be expired — run `cas cloud login` to re-authenticate."
                            );
                        }
                        FetchTeamsOutcome::AuthFailed => {}
                        FetchTeamsOutcome::NetworkError(msg) => {
                            tracing::warn!(
                                error = %msg,
                                "lazy /api/me refresh failed (non-fatal, continuing sync)"
                            );
                        }
                    }
                }
            }
        }
    }

    // T6: first-run backfill notice — runs even when the teams cache is
    // already fresh (cheap JSON read; gated by team_backfill_notified so it
    // is a no-op after the first time). Must run before execute_push so the
    // updated default_team_id is visible to the syncing stores opened inside
    // execute_push via open_store → active_team_id().
    if !args.dry_run {
        let outcome = maybe_apply_team_backfill();
        if emit_output {
            print_backfill_notice(cli, &outcome);
        }

        // cas-c117 (operator directive): the team identity is already known
        // locally by this point — `/api/me` filled `teams[]` and the backfill
        // above set `default_team_id` — so a logged-in user must not have to
        // run `cas cloud team set` / `team auto on` before their project is
        // scoped to their team. Adopt it here, BEFORE the personal-scope
        // notice (which then correctly says nothing) and before execute_push
        // opens the syncing stores that read `active_team_id()`.
        match crate::cloud::maybe_adopt_team_scope(cas_root) {
            Ok(adoption) if emit_output => print_team_scope_adoption(cli, &adoption),
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "could not adopt the resolvable team scope");
            }
        }

        match maybe_mark_personal_scope_notice(cas_root) {
            Ok(Some(notice)) if emit_output => print_personal_scope_notice(cli, &notice),
            Ok(Some(_)) => {}
            Ok(None) => {}
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "could not persist personal-scope team availability notice"
                );
            }
        }
    }

    // cas-c117: make the project↔team registration explicit and verified
    // BEFORE anything prints "✓ Push complete". Registration used to be a
    // side effect of a non-empty team push, so a machine with nothing queued
    // synced "successfully" while the server never learned about the project,
    // and `cas cloud team-memories` then told the user to run the sync they
    // had just run. This step either confirms the registration or fails the
    // whole command with the real reason.
    if !args.dry_run {
        let cloud_config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)?;
        ensure_team_project_registration_with_output(
            &cloud_config,
            cas_root,
            cli,
            args.full,
            emit_output,
        )?;
    }

    let operation_output = emit_output && (cli.json || args.dry_run);
    summaries.push(execute_push_with_output(
        &CloudPushArgs {
            entries_only: false,
            tasks_only: false,
            dry_run: args.dry_run,
            max_batches: None,
            rehome: args.rehome,
        },
        cli,
        cas_root,
        operation_output,
    )?);

    if !args.dry_run {
        // Drain the team queue before pulling — when a team is configured,
        // writes since the last sync were dual-enqueued by T3's syncing
        // wrappers; this is where the team rows reach the server. Team
        // push failure is isolated from the personal drain above (which
        // already succeeded by now) and from the pull below (best-effort).
        let cloud_config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)?;
        if let Some(team_summary) =
            execute_team_push_with_output(&cloud_config, cas_root, cli, emit_output && cli.json)?
        {
            if let Some(push_summary) = summaries.first_mut() {
                push_summary.merge_team_summary(&team_summary);
            }
        }

        // Personal pull AND team pull happen transitively here:
        // `execute_pull` invokes `execute_team_pull` at its tail when an
        // active team is configured (cas-6ec7). `execute_sync` does NOT
        // call `execute_team_pull` itself — duplicating the call would
        // fire the team-pull HTTP request twice per sync (the second
        // call returns 0 rows because the first advanced the `since=`
        // watermark, but the wasted round-trip is still observable). The
        // behavioral wiremock test in `team_pull_wiring_test.rs`
        // (`execute_sync_hits_each_pull_endpoint_exactly_once_when_team_configured`)
        // locks this invariant in with `.expect(1)` on both endpoints.
        let pull_summary = execute_pull_with_output(
            &CloudPullArgs {
                entries_only: false,
                tasks_only: false,
                full: args.full,
            },
            cli,
            cas_root,
            operation_output,
        )?;

        // T5: distilled knowledge rides the same sync. Kept last and
        // non-fatal — entries and tasks have already landed by here, and a
        // cloud without knowledge support must not fail the whole command.
        let pull_summary = match sync_project_knowledge_with_output(cli, cas_root, operation_output)
        {
            Ok(knowledge) => pull_summary.with_knowledge(knowledge),
            Err(e) => {
                tracing::warn!(error = %e, "knowledge sync failed (non-fatal)");
                pull_summary
            }
        };
        summaries.push(pull_summary);
    }

    if emit_output && !cli.json && !args.dry_run {
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, ActiveTheme::default());
        for summary in &summaries {
            render_sync_summary(&mut fmt, summary, cli.verbose)?;
        }
    }

    Ok(summaries)
}

/// Metadata key recording a confirmed project↔team registration, so the
/// steady-state sync pays no extra round-trip. Scoped by team AND canonical
/// id: re-homing the project or switching teams must re-verify.
fn team_registration_metadata_key(team_id: &str, canonical_id: &str) -> String {
    format!("team_project_registered_{team_id}_{canonical_id}")
}

/// Ensure this project is registered with the active team before the sync
/// reports success (cas-c117).
///
/// No-op when no team is configured, when the user is not logged in (the
/// personal push that follows reports that far more clearly), or when a
/// previous sync already confirmed the registration — unless `force` (i.e.
/// `cas cloud sync --full`) asks for a fresh verification.
///
/// Contract, and the whole point of the task: this returns `Err` — failing
/// the entire `cas cloud sync` with a non-zero exit — whenever the server
/// does not end up listing the project. A green sync now implies the project
/// is genuinely registered.
///
/// `pub` + `#[doc(hidden)]` so `cas-cli/tests/team_registration_test.rs` can
/// drive it against a wiremock server. Not intended for external use.
#[doc(hidden)]
pub fn ensure_team_project_registration(
    cloud_config: &CloudConfig,
    cas_root: &Path,
    cli: &Cli,
    force: bool,
) -> anyhow::Result<()> {
    ensure_team_project_registration_with_output(cloud_config, cas_root, cli, force, true)
}

fn ensure_team_project_registration_with_output(
    cloud_config: &CloudConfig,
    cas_root: &Path,
    cli: &Cli,
    force: bool,
    emit_output: bool,
) -> anyhow::Result<()> {
    let Some(team_id) = cloud_config.active_team_id() else {
        return Ok(());
    };
    let team_id = team_id.to_string();

    let Some(token) = cloud_config.token.as_deref() else {
        // Not logged in: `execute_push` reports this immediately after us with
        // the canonical message. Registering is impossible either way, and
        // duplicating the login error here would only be noise.
        return Ok(());
    };

    // Same resolver the team push uses (`cloud/syncer/team_push.rs`), so the
    // bucket we register is exactly the bucket rows are pushed into.
    let canonical_id = crate::cloud::resolve_canonical_id_for_sync(cas_root).map_err(|error| {
        anyhow::anyhow!(
            "Cannot register this project with team {team_id}: {error}. Run `cas cloud project set <canonical-id>` (see `cas cloud projects`)."
        )
    })?;

    // Best-effort cache. A queue that cannot be opened just means we verify
    // over the network every sync — correctness never depends on the cache.
    let queue = SyncQueue::open(cas_root).ok().and_then(|q| {
        // `init()` is idempotent; without it a brand-new root has no
        // sync_metadata table to read.
        if let Err(e) = q.init() {
            tracing::warn!(error = %e, "team registration: sync queue init failed; skipping cache");
            return None;
        }
        Some(q)
    });
    let cache_key = team_registration_metadata_key(&team_id, &canonical_id);
    if !force {
        if let Some(q) = queue.as_ref() {
            if matches!(q.get_metadata(&cache_key), Ok(Some(_))) {
                return Ok(());
            }
        }
    }

    // cas-8ca5 contract §5 — same remote the team push sends, so the server's
    // resolver maps us onto the team's existing bucket rather than forking a
    // new one.
    let git_remote = crate::cloud::normalized_git_remote_for_push(cas_root);

    let pinned_canonical_id = crate::cloud::canonical_id_from_config_toml(cas_root);
    let registration =
        crate::cloud::TeamRegistration::new(&cloud_config.endpoint, token, &team_id, &canonical_id)
            .with_git_remote(git_remote.as_deref())
            .with_pinned_canonical_id(pinned_canonical_id.as_deref());

    match registration.ensure() {
        Ok(outcome) => {
            let effective_id = match outcome.adopted_canonical_id() {
                Some(adopted) => {
                    crate::cloud::set_canonical_id_in_config_toml(cas_root, adopted)?;
                    crate::cloud::invalidate_cached_project_id();
                    tracing::info!(
                        sent_canonical_id = %canonical_id,
                        adopted_canonical_id = %adopted,
                        "adopted server-resolved canonical project id during registration"
                    );
                    adopted.to_string()
                }
                None => canonical_id.clone(),
            };
            if let Some(q) = queue.as_ref() {
                let effective_cache_key = team_registration_metadata_key(&team_id, &effective_id);
                let _ = q.set_metadata(&effective_cache_key, &chrono::Utc::now().to_rfc3339());
            }
            if emit_output {
                report_team_registration(cli, &team_id, &effective_id, &outcome)?;
            }
            Ok(())
        }
        Err(failure) => {
            tracing::error!(
                team_id = %team_id,
                canonical_id = %canonical_id,
                interaction = %failure.interaction,
                "project could not be registered with the team; failing the sync"
            );
            if emit_output && cli.json {
                println!(
                    "{}",
                    serde_json::json!({
                        "team_registration": {
                            "team_id": team_id,
                            "canonical_id": canonical_id,
                            "registered": false,
                            "reason": failure.reason,
                            "interaction": failure.interaction,
                        }
                    })
                );
            }
            Err(anyhow::anyhow!(
                "Sync aborted: this project is not registered with your team, so team \
                 memories and team pushes would silently do nothing.\n  {failure}"
            ))
        }
    }
}

fn report_team_registration(
    cli: &Cli,
    team_id: &str,
    canonical_id: &str,
    outcome: &crate::cloud::RegistrationOutcome,
) -> anyhow::Result<()> {
    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "team_registration": {
                    "team_id": team_id,
                    "canonical_id": canonical_id,
                    "registered": true,
                    "newly_registered": outcome.newly_registered(),
                    "adopted_canonical_id": outcome.adopted_canonical_id(),
                    "project_uuid": outcome.project_uuid(),
                }
            })
        );
        return Ok(());
    }

    // Only the state change is worth a line; a project that was already
    // registered stays quiet so routine syncs keep their current output.
    if outcome.newly_registered() {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let success_color = fmt.theme().palette.status_success;
        fmt.write_colored("  \u{2713} ", success_color)?;
        fmt.write_raw(&format!(
            "Registered project {canonical_id} with team {team_id}"
        ))?;
        fmt.newline()?;
    } else if outcome.adopted_canonical_id().is_some() {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let success_color = fmt.theme().palette.status_success;
        fmt.write_colored("  ✓ ", success_color)?;
        fmt.write_raw(&format!(
            "Adopted team project id {canonical_id} (server-resolved existing bucket)"
        ))?;
        fmt.newline()?;
    }
    Ok(())
}

/// Drain the team queue into `POST /api/teams/{uuid}/sync/push` when a
/// team is configured. No-op when no active team.
///
/// Contract: always returns `Ok(())` — team-push failures are reported
/// via `report_team_push_*` and isolated from the surrounding sync so
/// the personal drain already done stays, and the pull that follows
/// still runs. Items that fail to push remain in the team queue
/// (re-enqueued by `push_team` itself) for the next sync cycle.
///
/// `pub` so `cas-cli/tests/team_sync_test.rs` can exercise the helper
/// directly with a wiremock server. Not intended for external use.
#[doc(hidden)]
pub fn execute_team_push(
    cloud_config: &CloudConfig,
    cas_root: &Path,
    cli: &Cli,
) -> anyhow::Result<()> {
    execute_team_push_with_output(cloud_config, cas_root, cli, true).map(|_| ())
}

fn execute_team_push_with_output(
    cloud_config: &CloudConfig,
    cas_root: &Path,
    cli: &Cli,
    emit_output: bool,
) -> anyhow::Result<Option<SyncSummary>> {
    let Some(team_id) = cloud_config.active_team_id() else {
        return Ok(None);
    };
    let team_id = team_id.to_string();

    let queue = match crate::cloud::SyncQueue::open(cas_root) {
        Ok(q) => {
            if let Err(e) = q.init() {
                tracing::warn!(
                    target: "cas::sync",
                    error = %e,
                    "team sync queue init failed; draining aborted",
                );
                // Isolation contract: reporter errors must not escape.
                if emit_output {
                    let _ =
                        report_team_push_error(cli, &format!("Team sync queue init failed: {e}"));
                }
                return Ok(Some(SyncSummary::push(
                    &crate::cloud::SyncResult {
                        errors: vec![format!("Team sync queue init failed: {e}")],
                        ..Default::default()
                    },
                    crate::cloud::PushScope::All,
                    None,
                )));
            }
            q
        }
        Err(e) => {
            if emit_output {
                let _ = report_team_push_error(cli, &format!("Could not open sync queue: {e}"));
            }
            return Ok(Some(SyncSummary::push(
                &crate::cloud::SyncResult {
                    errors: vec![format!("Could not open sync queue: {e}")],
                    ..Default::default()
                },
                crate::cloud::PushScope::All,
                None,
            )));
        }
    };

    let project_id = match crate::cloud::resolve_canonical_id_for_sync(cas_root) {
        Ok(id) => id,
        Err(e) => {
            if emit_output {
                let _ = report_team_push_error(cli, &e.to_string());
            }
            return Ok(Some(SyncSummary::push(
                &crate::cloud::SyncResult {
                    errors: vec![e.to_string()],
                    ..Default::default()
                },
                crate::cloud::PushScope::All,
                None,
            )));
        }
    };
    let syncer = crate::cloud::CloudSyncer::new_for_project(
        std::sync::Arc::new(queue),
        cloud_config.clone(),
        crate::cloud::CloudSyncerConfig::default(),
        project_id,
        cas_root,
    );

    // `let _ =` on reporter calls: a formatter/IO error from the display
    // path must not propagate out and block the caller's pull step.
    match syncer.push_team(&team_id) {
        Ok(result) => {
            let summary = SyncSummary::push(&result, crate::cloud::PushScope::All, None);
            if emit_output {
                if result.errors.is_empty() {
                    let _ = report_team_push_result(cli, &team_id, &result);
                } else {
                    let _ = report_team_push_partial(cli, &team_id, &result);
                }
            }
            Ok(Some(summary))
        }
        Err(e) => {
            if emit_output {
                let _ = report_team_push_error(cli, &format!("Team push failed: {e}"));
            }
            Ok(Some(SyncSummary::push(
                &crate::cloud::SyncResult {
                    errors: vec![format!("Team push failed: {e}")],
                    ..Default::default()
                },
                crate::cloud::PushScope::All,
                None,
            )))
        }
    }
}

fn report_team_push_result(
    cli: &Cli,
    team_id: &str,
    result: &crate::cloud::SyncResult,
) -> anyhow::Result<()> {
    if cli.json {
        println!("{}", team_push_json(team_id, result, &[]));
    } else {
        // `total_pushed()` sums all entity types — not just the four most
        // common kinds — so an otherwise quiet team push stays quiet unless
        // it actually moved data.
        if result.total_pushed() > 0 {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            let summary = SyncSummary::push(result, crate::cloud::PushScope::All, None);
            render_sync_summary(&mut fmt, &summary, cli.verbose)?;
        }
    }
    Ok(())
}

/// Shared JSON shape for `report_team_push_{result,partial}` — consumers
/// see a consistent `{team_push: {...}}` object whether the push fully
/// succeeded or partially failed. `errors` is always present (empty for
/// full success), and every `pushed_*` count is always present.
fn team_push_json(
    team_id: &str,
    result: &crate::cloud::SyncResult,
    extra_errors: &[String],
) -> serde_json::Value {
    let mut errors = result.concise_errors();
    errors.extend(extra_errors.iter().cloned());
    serde_json::json!({
        "team_push": {
            "team_id": team_id,
            "pushed_entries": result.pushed_entries,
            "pushed_tasks": result.pushed_tasks,
            "pushed_rules": result.pushed_rules,
            "pushed_skills": result.pushed_skills,
            "pushed_sessions": result.pushed_sessions,
            "pushed_verifications": result.pushed_verifications,
            "pushed_events": result.pushed_events,
            "pushed_prompts": result.pushed_prompts,
            "pushed_file_changes": result.pushed_file_changes,
            "pushed_commit_links": result.pushed_commit_links,
            "pushed_agents": result.pushed_agents,
            "pushed_worktrees": result.pushed_worktrees,
            "conflicts_resolved": result.conflicts_resolved,
            "conflicts_resolved_local": result.conflicts_resolved_local,
            "conflicts_resolved_remote": result.conflicts_resolved_remote,
            "healed_task_dependencies_to_cloud": result.healed_task_dependencies_to_cloud,
            "healed_task_dependencies_from_cloud": result.healed_task_dependencies_from_cloud,
            "deleted_task_dependencies": result.deleted_task_dependencies,
            "skipped_task_dependencies_by_tombstone":
                result.skipped_task_dependencies_by_tombstone,
            "total_pushed": result.total_pushed(),
            "duration_ms": result.duration_ms,
            "errors": errors,
        }
    })
}

fn report_team_push_partial(
    cli: &Cli,
    team_id: &str,
    result: &crate::cloud::SyncResult,
) -> anyhow::Result<()> {
    if cli.json {
        // Same shape as the full-success path so JSON consumers can
        // always read pushed counts regardless of outcome.
        println!("{}", team_push_json(team_id, result, &[]));
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let summary = SyncSummary::push(result, crate::cloud::PushScope::All, None);
        render_sync_summary(&mut fmt, &summary, cli.verbose)?;
    }
    Ok(())
}

fn report_team_push_error(cli: &Cli, msg: &str) -> anyhow::Result<()> {
    if cli.json {
        // Empty SyncResult + the single fatal error as a string — keeps
        // shape consistent with success/partial paths.
        let empty = crate::cloud::SyncResult::default();
        println!(
            "{}",
            team_push_json("", &empty, std::slice::from_ref(&msg.to_string()))
        );
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let warning_color = fmt.theme().palette.status_warning;
        fmt.write_colored("  \u{26A0} ", warning_color)?;
        fmt.write_raw(msg)?;
        fmt.newline()?;
    }
    Ok(())
}

/// Pull team data into the local stores from `GET /api/teams/{uuid}/sync/pull`
/// when a team is configured. No-op when no active team.
///
/// Contract: always returns `Ok(())` — team-pull failures are reported via
/// `report_team_pull_*` and isolated from the surrounding sync so the
/// personal pull that ran just before stays, and any caller chained after
/// (e.g. `execute_sync` exit) still completes cleanly. Mirrors the isolation
/// contract of `execute_team_push` (cli/cloud.rs:1313).
///
/// Signature note: `pull_team` currently takes 4 stores (entries / tasks /
/// rules / skills) — NOT the full 9-store set that personal `pull` takes.
/// Per task cas-6ec7 spec, this helper preserves that parity. Extending
/// `pull_team` to specs / events / prompts / file_changes / commit_links is
/// a separate scope expansion.
///
/// `pub` so `cas-cli/tests/team_pull_wiring_test.rs` can exercise the helper
/// directly with a wiremock server, matching the precedent set by
/// `execute_team_push` for `team_sync_test.rs`. Not intended for external
/// (public-API) use.
#[doc(hidden)]
pub fn execute_team_pull(
    cloud_config: &CloudConfig,
    cas_root: &Path,
    cli: &Cli,
) -> anyhow::Result<()> {
    execute_team_pull_with_output(cloud_config, cas_root, cli, true).map(|_| ())
}

fn execute_team_pull_with_output(
    cloud_config: &CloudConfig,
    cas_root: &Path,
    cli: &Cli,
    emit_output: bool,
) -> anyhow::Result<Option<SyncSummary>> {
    let Some(team_id) = cloud_config.active_team_id() else {
        return Ok(None);
    };
    let team_id = team_id.to_string();

    let queue = match crate::cloud::SyncQueue::open(cas_root) {
        Ok(q) => {
            if let Err(e) = q.init() {
                tracing::warn!(
                    target: "cas::sync",
                    error = %e,
                    "team sync queue init failed; team pull aborted",
                );
                // Isolation contract: reporter errors must not escape.
                if emit_output {
                    let _ =
                        report_team_pull_error(cli, &format!("Team sync queue init failed: {e}"));
                }
                return Ok(Some(SyncSummary::pull(
                    &crate::cloud::SyncResult {
                        errors: vec![format!("Team sync queue init failed: {e}")],
                        ..Default::default()
                    },
                    true,
                )));
            }
            q
        }
        Err(e) => {
            if emit_output {
                let _ = report_team_pull_error(cli, &format!("Could not open sync queue: {e}"));
            }
            return Ok(Some(SyncSummary::pull(
                &crate::cloud::SyncResult {
                    errors: vec![format!("Could not open sync queue: {e}")],
                    ..Default::default()
                },
                true,
            )));
        }
    };

    // Stores synced by `pull_team`: entries / tasks / rules / skills (only).
    // Per cas-6ec7 spec, this is intentional parity with the current
    // `pull_team` signature — adding the remaining 5 entity kinds is a
    // separate scope expansion.
    //
    // cas-7fbb: *_local openers — pull must not re-enqueue pulled rows.
    let store = match open_store_local(cas_root) {
        Ok(s) => s,
        Err(e) => {
            if emit_output {
                let _ = report_team_pull_error(cli, &format!("Could not open entry store: {e}"));
            }
            return Ok(Some(SyncSummary::pull(
                &crate::cloud::SyncResult {
                    errors: vec![format!("Could not open entry store: {e}")],
                    ..Default::default()
                },
                true,
            )));
        }
    };
    let task_store = match open_task_store_local(cas_root) {
        Ok(s) => s,
        Err(e) => {
            if emit_output {
                let _ = report_team_pull_error(cli, &format!("Could not open task store: {e}"));
            }
            return Ok(Some(SyncSummary::pull(
                &crate::cloud::SyncResult {
                    errors: vec![format!("Could not open task store: {e}")],
                    ..Default::default()
                },
                true,
            )));
        }
    };
    let rule_store = match open_rule_store_local(cas_root) {
        Ok(s) => s,
        Err(e) => {
            if emit_output {
                let _ = report_team_pull_error(cli, &format!("Could not open rule store: {e}"));
            }
            return Ok(Some(SyncSummary::pull(
                &crate::cloud::SyncResult {
                    errors: vec![format!("Could not open rule store: {e}")],
                    ..Default::default()
                },
                true,
            )));
        }
    };
    let skill_store = match open_skill_store_local(cas_root) {
        Ok(s) => s,
        Err(e) => {
            if emit_output {
                let _ = report_team_pull_error(cli, &format!("Could not open skill store: {e}"));
            }
            return Ok(Some(SyncSummary::pull(
                &crate::cloud::SyncResult {
                    errors: vec![format!("Could not open skill store: {e}")],
                    ..Default::default()
                },
                true,
            )));
        }
    };

    // cas-53d5: `pull_team` now takes the canonical project_id explicitly
    // so its watermark is scoped per (team_id, project_id). Resolve here at
    // the caller and bail with the same isolation contract if we can't —
    // pull_team would otherwise have failed at its old internal resolve.
    let project_id = match crate::cloud::resolve_canonical_id_for_sync(cas_root) {
        Ok(id) => id,
        Err(e) => {
            if emit_output {
                let _ = report_team_pull_error(cli, &e.to_string());
            }
            return Ok(Some(SyncSummary::pull(
                &crate::cloud::SyncResult {
                    errors: vec![e.to_string()],
                    ..Default::default()
                },
                true,
            )));
        }
    };
    let syncer = crate::cloud::CloudSyncer::new_for_project(
        std::sync::Arc::new(queue),
        cloud_config.clone(),
        crate::cloud::CloudSyncerConfig::default(),
        project_id.clone(),
        cas_root,
    );

    // `let _ =` on reporter calls: a formatter/IO error from the display
    // path must not propagate out and block subsequent caller steps.
    match syncer.pull_team(
        &team_id,
        &project_id,
        store.as_ref(),
        task_store.as_ref(),
        rule_store.as_ref(),
        skill_store.as_ref(),
    ) {
        Ok(result) => {
            let summary = SyncSummary::pull(&result, true);
            if emit_output {
                if result.errors.is_empty() {
                    let _ = report_team_pull_result(cli, &team_id, &result);
                } else {
                    let _ = report_team_pull_partial(cli, &team_id, &result);
                }
            }
            Ok(Some(summary))
        }
        Err(e) => {
            if emit_output {
                let _ = report_team_pull_error(cli, &format!("Team pull failed: {e}"));
            }
            Ok(Some(SyncSummary::pull(
                &crate::cloud::SyncResult {
                    errors: vec![format!("Team pull failed: {e}")],
                    ..Default::default()
                },
                true,
            )))
        }
    }
}

/// Shared JSON shape for `report_team_pull_{result,partial,error}` —
/// consumers see a consistent `{team_pull: {...}}` object regardless of
/// outcome. Mirrors `team_push_json`'s shape so JSON consumers can branch
/// on the wrapper key.
fn team_pull_json(
    team_id: &str,
    result: &crate::cloud::SyncResult,
    extra_errors: &[String],
) -> serde_json::Value {
    let mut errors = result.errors.clone();
    errors.extend(extra_errors.iter().cloned());
    let mut output = serde_json::json!({
        "team_pull": {
            "team_id": team_id,
            "pulled_entries": result.pulled_entries,
            "pulled_tasks": result.pulled_tasks,
            "pulled_rules": result.pulled_rules,
            "pulled_skills": result.pulled_skills,
            "conflicts_resolved": result.conflicts_resolved,
            "conflicts_resolved_local": result.conflicts_resolved_local,
            "conflicts_resolved_remote": result.conflicts_resolved_remote,
            "healed_task_dependencies_to_cloud": result.healed_task_dependencies_to_cloud,
            "healed_task_dependencies_from_cloud": result.healed_task_dependencies_from_cloud,
            "deleted_task_dependencies": result.deleted_task_dependencies,
            "skipped_task_dependencies_by_tombstone":
                result.skipped_task_dependencies_by_tombstone,
            "duration_ms": result.duration_ms,
            "errors": errors,
        }
    });
    if !result.task_status_transitions.is_empty() {
        output["team_pull"]["task_status_transitions"] =
            serde_json::to_value(&result.task_status_transitions).unwrap_or_default();
    }
    output
}

fn report_team_pull_result(
    cli: &Cli,
    team_id: &str,
    result: &crate::cloud::SyncResult,
) -> anyhow::Result<()> {
    if cli.json {
        println!("{}", team_pull_json(team_id, result, &[]));
    } else {
        // Suppress no-op output when nothing was pulled — keeps the human
        // sync log uncluttered for the steady-state case (matches the
        // `total_pushed() > 0` guard in `report_team_push_result`).
        let total = result.pulled_entries
            + result.pulled_tasks
            + result.pulled_rules
            + result.pulled_skills;
        if total > 0 {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            let success_color = fmt.theme().palette.status_success;
            fmt.write_colored("  \u{2713} ", success_color)?;
            fmt.write_raw(&format!(
                "Team pull: {} entries, {} tasks, {} rules, {} skills ({} total)",
                result.pulled_entries,
                result.pulled_tasks,
                result.pulled_rules,
                result.pulled_skills,
                total,
            ))?;
            fmt.newline()?;
        }
        if let Some(summary) = task_transition_summary(result) {
            let theme = ActiveTheme::default();
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.write_raw(&summary)?;
            fmt.newline()?;
        }
    }
    Ok(())
}

fn report_team_pull_partial(
    cli: &Cli,
    team_id: &str,
    result: &crate::cloud::SyncResult,
) -> anyhow::Result<()> {
    if cli.json {
        // Same shape as the full-success path so JSON consumers can always
        // read pulled counts regardless of outcome.
        println!("{}", team_pull_json(team_id, result, &[]));
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let warning_color = fmt.theme().palette.status_warning;
        fmt.write_colored("  \u{26A0} ", warning_color)?;
        fmt.write_raw(&format!(
            "Team pull encountered {} error(s); partial results applied",
            result.errors.len()
        ))?;
        fmt.newline()?;
        for err in &result.errors {
            fmt.write_muted("    - ")?;
            fmt.write_raw(err)?;
            fmt.newline()?;
        }
    }
    Ok(())
}

fn report_team_pull_error(cli: &Cli, msg: &str) -> anyhow::Result<()> {
    if cli.json {
        // Empty SyncResult + the single fatal error as a string — keeps
        // shape consistent with success/partial paths.
        let empty = crate::cloud::SyncResult::default();
        println!(
            "{}",
            team_pull_json("", &empty, std::slice::from_ref(&msg.to_string()))
        );
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        let warning_color = fmt.theme().palette.status_warning;
        fmt.write_colored("  \u{26A0} ", warning_color)?;
        fmt.write_raw(msg)?;
        fmt.newline()?;
    }
    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// PROJECTS - List team projects
// ═══════════════════════════════════════════════════════════════════════════════

fn execute_projects(args: &CloudProjectsArgs, cli: &Cli, cas_root: &Path) -> anyhow::Result<()> {
    let config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)?;
    let token = config
        .token
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not logged in. Run 'cas login' first"))?;

    // Resolve team_id: --team flag overrides config
    let team_id = args
        .team
        .as_deref()
        .or(config.team_id.as_deref())
        .or(config.team_slug.as_deref());

    let team_id = match team_id {
        Some(id) => id,
        None => {
            if cli.json {
                println!(r#"{{"status":"error","message":"No team configured"}}"#);
            } else {
                let theme = ActiveTheme::default();
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme);
                let warning_color = fmt.theme().palette.status_warning;
                fmt.write_colored("  \u{25CF} ", warning_color)?;
                fmt.write_raw("No team configured. Run ")?;
                fmt.write_accent("cas cloud team set <uuid>")?;
                fmt.write_raw(" first.")?;
                fmt.newline()?;
            }
            return Ok(());
        }
    };

    let url = format!("{}/api/teams/{}/projects", config.endpoint, team_id);

    match ureq::get(&url)
        .set("Authorization", &format!("Bearer {token}"))
        .call()
    {
        Ok(resp) => {
            let body: crate::cloud::TeamProjectsResponse = resp.into_json()?;

            if cli.json {
                println!("{}", serde_json::to_string(&body.projects)?);
            } else {
                let theme = ActiveTheme::default();
                let mut out = io::stdout();
                let mut fmt = Formatter::stdout(&mut out, theme);

                fmt.newline()?;
                let team_display = args
                    .team
                    .as_deref()
                    .or(config.team_slug.as_deref())
                    .unwrap_or(team_id);
                fmt.write_muted("  Team: ")?;
                fmt.write_accent(team_display)?;
                fmt.newline()?;
                fmt.newline()?;

                if body.projects.is_empty() {
                    fmt.write_muted("  No projects found.")?;
                    fmt.newline()?;
                } else {
                    // Calculate column widths for aligned output
                    let max_name = body
                        .projects
                        .iter()
                        .map(|p| p.name.len())
                        .max()
                        .unwrap_or(0)
                        .max(4);
                    let max_canonical = body
                        .projects
                        .iter()
                        .map(|p| p.canonical_id.len())
                        .max()
                        .unwrap_or(0)
                        .max(4);

                    for project in &body.projects {
                        let contrib_label = if project.contributor_count == 1 {
                            "contributor"
                        } else {
                            "contributors"
                        };
                        let mem_label = if project.memory_count == 1 {
                            "memory"
                        } else {
                            "memories"
                        };
                        fmt.write_raw(&format!(
                            "    {:<name_w$}   {:<canonical_w$}   {} {:<14}  {} {}",
                            project.name,
                            project.canonical_id,
                            project.contributor_count,
                            contrib_label,
                            project.memory_count,
                            mem_label,
                            name_w = max_name,
                            canonical_w = max_canonical,
                        ))?;
                        fmt.newline()?;
                    }
                }
                fmt.newline()?;
            }
        }
        Err(ureq::Error::Status(401, _)) => {
            if cli.json {
                println!(r#"{{"status":"error","message":"Invalid or expired token"}}"#);
            } else {
                let theme = ActiveTheme::default();
                let mut err = io::stderr();
                let mut fmt = Formatter::stdout(&mut err, theme);
                let error_color = fmt.theme().palette.status_error;
                fmt.write_colored("  \u{2717} ", error_color)?;
                fmt.write_raw("Session expired")?;
                fmt.newline()?;
                fmt.write_raw("  Run ")?;
                fmt.write_accent("cas login")?;
                fmt.write_raw(" to re-authenticate")?;
                fmt.newline()?;
            }
        }
        Err(ureq::Error::Status(403, _)) => {
            anyhow::bail!("You're not a member of this team.");
        }
        Err(e) => {
            anyhow::bail!("Failed to fetch projects: {e}");
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// TEAM MEMORIES
// ═══════════════════════════════════════════════════════════════════════════════

fn execute_team_memories(
    args: &CloudTeamMemoriesArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    use crate::cloud::{TeamMemoriesResponse, TeamProjectsResponse};
    use crate::ui::components::{Spinner, clear_inline, render_inline_view};

    let mut config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)?;

    let team_id = config
        .team_id
        .as_ref()
        .ok_or_else(|| {
            anyhow::anyhow!("No team configured. Run `cas cloud team set <uuid>` first.")
        })?
        .clone();

    let canonical_id = crate::cloud::resolve_canonical_id_for_sync(cas_root)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;

    let token = config
        .token
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Not logged in. Run 'cas login' first."))?
        .clone();

    let theme = ActiveTheme::default();
    let prev_lines = if !cli.json {
        let spinner = Spinner::new("Pulling team memories...");
        render_inline_view(&spinner, &theme)?
    } else {
        0u16
    };

    // Step 1: Find the project UUID by listing team projects
    let projects_url = format!("{}/api/teams/{}/projects", config.endpoint, team_id);
    let projects_resp = ureq::get(&projects_url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(30))
        .call();

    let projects_body: TeamProjectsResponse = match projects_resp {
        Ok(resp) => resp.into_json()?,
        Err(ureq::Error::Status(401, _)) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("Session expired. Run `cas login` to re-authenticate.");
        }
        Err(ureq::Error::Status(403, _)) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("You're not a member of this team.");
        }
        Err(e) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("Failed to list team projects: {e}");
        }
    };

    let project = projects_body
        .projects
        .iter()
        .find(|p| project_ids_match(&p.canonical_id, &canonical_id));

    let project_uuid = match project {
        Some(p) => p.id.clone(),
        None => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            // cas-c117: the old wording ("run `cas cloud sync` to register
            // it") was circular — the user reaches this line precisely after
            // a green sync. Sync now registers the project or fails loudly,
            // so the only remaining explanation is a bucket mismatch: name
            // the ids involved instead of repeating the instruction.
            anyhow::bail!(
                "Project '{canonical_id}' is not registered with team {team_id} on {}. \
                 `cas cloud sync` registers it (and now fails loudly if the server refuses), \
                 so if a sync just succeeded this project is most likely pinned to a different \
                 bucket than the team's. Compare `cas cloud team show` with `cas cloud projects` \
                 and pin the right one with `cas cloud project set <canonical-id>`.",
                config.endpoint
            );
        }
    };

    // Step 2: Fetch team memories for this project
    let mut memories_url = format!(
        "{}/api/teams/{}/projects/{}/memories",
        config.endpoint, team_id, project_uuid
    );

    if !args.full {
        if let Some(since) = config.get_team_memory_sync(&canonical_id) {
            memories_url = format!("{memories_url}?since={since}");
        }
    }

    let memories_resp = ureq::get(&memories_url)
        .set("Authorization", &format!("Bearer {token}"))
        .timeout(Duration::from_secs(60))
        .call();

    let body: TeamMemoriesResponse = match memories_resp {
        Ok(resp) => resp.into_json()?,
        Err(ureq::Error::Status(401, _)) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("Session expired. Run `cas login` to re-authenticate.");
        }
        Err(ureq::Error::Status(403, _)) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("You're not a member of this team.");
        }
        Err(ureq::Error::Status(404, _)) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("Project not found in this team.");
        }
        Err(e) => {
            if prev_lines > 0 {
                clear_inline(prev_lines)?;
            }
            anyhow::bail!("Failed to fetch team memories: {e}");
        }
    };

    let entry_count = body.memories.entries.len();
    let rule_count = body.memories.rules.len();
    let skill_count = body.memories.skills.len();
    let contributor_count = body.contributors.len();

    // Dry run: just show counts
    if args.dry_run {
        if prev_lines > 0 {
            clear_inline(prev_lines)?;
        }

        if cli.json {
            println!(
                "{}",
                serde_json::json!({
                    "dry_run": true,
                    "entries": entry_count,
                    "rules": rule_count,
                    "skills": skill_count,
                    "contributors": contributor_count,
                })
            );
        } else {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            fmt.write_accent("  \u{2192} ")?;
            fmt.write_raw(&format!(
                "Would pull: {} entries, {} rules, {} skills from {} contributors",
                entry_count, rule_count, skill_count, contributor_count
            ))?;
            fmt.newline()?;
        }
        return Ok(());
    }

    // Check if there's anything to merge
    if entry_count == 0 && rule_count == 0 && skill_count == 0 {
        if prev_lines > 0 {
            clear_inline(prev_lines)?;
        }
        if cli.json {
            println!(r#"{{"status":"ok","message":"up_to_date"}}"#);
        } else {
            let mut out = io::stdout();
            let mut fmt = Formatter::stdout(&mut out, theme);
            let success_color = fmt.theme().palette.status_success;
            fmt.write_colored("  \u{2713} ", success_color)?;
            fmt.write_raw("Team memories are up to date.")?;
            fmt.newline()?;
        }
        return Ok(());
    }

    // Merge into local stores using LWW.
    // cas-7fbb: apply remote team memories without re-enqueueing to SyncQueue.
    let store = open_store_local(cas_root)?;
    let rule_store = open_rule_store_local(cas_root)?;
    let skill_store = open_skill_store_local(cas_root)?;

    let mut entries_merged = 0usize;
    let mut entries_skipped = 0usize;
    let mut rules_merged = 0usize;
    let mut rules_skipped = 0usize;
    let mut skills_merged = 0usize;
    let mut skills_skipped = 0usize;

    // Merge entries (LWW by last_accessed or created)
    for entry in body.memories.entries {
        match store.get(&entry.id) {
            Ok(local) => {
                let local_time = local.last_accessed.unwrap_or(local.created);
                let remote_time = entry.last_accessed.unwrap_or(entry.created);
                if remote_time > local_time {
                    store.update(&entry)?;
                    entries_merged += 1;
                } else {
                    entries_skipped += 1;
                }
            }
            Err(_) => {
                store.add(&entry)?;
                entries_merged += 1;
            }
        }
    }

    // Merge rules (LWW by last_accessed or created)
    for rule in body.memories.rules {
        match rule_store.get(&rule.id) {
            Ok(local) => {
                let local_time = local.last_accessed.unwrap_or(local.created);
                let remote_time = rule.last_accessed.unwrap_or(rule.created);
                if remote_time > local_time {
                    rule_store.update(&rule)?;
                    rules_merged += 1;
                } else {
                    rules_skipped += 1;
                }
            }
            Err(_) => {
                rule_store.add(&rule)?;
                rules_merged += 1;
            }
        }
    }

    // Merge skills (LWW by updated_at)
    for skill in body.memories.skills {
        match skill_store.get(&skill.id) {
            Ok(local) => {
                if skill.updated_at > local.updated_at {
                    skill_store.update(&skill)?;
                    skills_merged += 1;
                } else {
                    skills_skipped += 1;
                }
            }
            Err(_) => {
                skill_store.add(&skill)?;
                skills_merged += 1;
            }
        }
    }

    // Save sync timestamp
    if let Some(pulled_at) = &body.pulled_at {
        config.set_team_memory_sync(&canonical_id, pulled_at);
        config.save_to_cas_dir(cas_root)?;
    }

    if prev_lines > 0 {
        clear_inline(prev_lines)?;
    }

    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "status": "ok",
                "entries": { "merged": entries_merged, "skipped": entries_skipped },
                "rules": { "merged": rules_merged, "skipped": rules_skipped },
                "skills": { "merged": skills_merged, "skipped": skills_skipped },
                "contributors": contributor_count,
            })
        );
    } else {
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.success("Team memories synced")?;
        if entries_merged > 0 {
            fmt.write_raw(&format!("    {} entries merged", entries_merged))?;
            fmt.newline()?;
        }
        if rules_merged > 0 {
            fmt.write_raw(&format!("    {} rules merged", rules_merged))?;
            fmt.newline()?;
        }
        if skills_merged > 0 {
            fmt.write_raw(&format!("    {} skills merged", skills_merged))?;
            fmt.newline()?;
        }
        if entries_skipped + rules_skipped + skills_skipped > 0 {
            fmt.write_muted(&format!(
                "    {} skipped (local newer)",
                entries_skipped + rules_skipped + skills_skipped
            ))?;
            fmt.newline()?;
        }
    }

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════════════
// PURGE-FOREIGN - Remove foreign-project entities and re-pull
// ═══════════════════════════════════════════════════════════════════════════════

/// Default age (in days) beyond which `last_pull_at` is considered stale and a
/// destructive purge is refused. A purge on a machine that has not pulled
/// recently re-pulls an out-of-date snapshot over freshly deleted local rows.
pub const PURGE_STALE_THRESHOLD_DAYS: i64 = 7;

/// Maximum rows printed per entity kind in the human-readable dry-run listing.
/// The full set is always available via `--json`.
const PURGE_DRY_RUN_PRINT_LIMIT: usize = 50;

/// One row that `cas cloud purge-foreign` would delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeEntity {
    /// Entity kind: "entry" | "task" | "rule" | "skill".
    pub kind: &'static str,
    pub id: String,
    /// Human label (title/name/first content line) — may be empty.
    pub label: String,
    /// Evidence that made this row purgeable. This is rendered in dry-run
    /// output so a destructive preview names the classifier it relied on.
    pub evidence: PurgeEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeEvidence {
    /// `origin_project` for the persisted project field, or `peer-evidence`
    /// when the doctor's cross-database activity classifier overrode a
    /// backfilled current-project value.
    pub source: &'static str,
    /// Stored origin project, or the peer project whose activity proves the
    /// row's home.
    pub project: String,
}

impl PurgeEntity {
    pub fn with_evidence(
        kind: &'static str,
        id: impl Into<String>,
        label: impl Into<String>,
        source: &'static str,
        project: impl Into<String>,
    ) -> Self {
        Self {
            kind,
            id: id.into(),
            label: label.into(),
            evidence: PurgeEvidence {
                source,
                project: project.into(),
            },
        }
    }

    fn evidence_label(&self) -> String {
        if self.evidence.source == "peer-evidence" {
            format!("peer-evidence + home project: {}", self.evidence.project)
        } else {
            format!("origin_project: {}", self.evidence.project)
        }
    }
}

/// The concrete set of rows a purge would delete.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PurgeDeleteSet {
    pub entries: Vec<PurgeEntity>,
    pub tasks: Vec<PurgeEntity>,
    pub rules: Vec<PurgeEntity>,
    pub skills: Vec<PurgeEntity>,
    /// Dependency edges attached to deleted foreign tasks; they have no
    /// user-facing title.
    pub dependencies: usize,
}

impl PurgeDeleteSet {
    pub fn total(&self) -> usize {
        self.entries.len() + self.tasks.len() + self.rules.len() + self.skills.len()
    }

    fn groups(&self) -> [(&'static str, &Vec<PurgeEntity>); 4] {
        [
            ("entries", &self.entries),
            ("tasks", &self.tasks),
            ("rules", &self.rules),
            ("skills", &self.skills),
        ]
    }

    pub(crate) fn to_json(&self) -> serde_json::Value {
        let render = |rows: &Vec<PurgeEntity>| {
            rows.iter()
                .map(|e| {
                    serde_json::json!({
                        "id": e.id,
                        "label": e.label,
                        "evidence": {
                            "source": e.evidence.source,
                            "project": e.evidence.project,
                        },
                    })
                })
                .collect::<Vec<_>>()
        };
        serde_json::json!({
            "entries": render(&self.entries),
            "tasks": render(&self.tasks),
            "rules": render(&self.rules),
            "skills": render(&self.skills),
            "dependencies": self.dependencies,
            "total": self.total(),
        })
    }
}

/// A foreign row found by doctor but intentionally retained by purge because
/// the classifier cannot safely decide ownership (for example, an id
/// collision or an unattributed replica).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeRetainedTask {
    pub id: String,
    pub title: String,
    pub reason: String,
    /// Home project from the cross-database classifier, when known. This is
    /// used only to print a scoped, operator-confirmed remote DELETE command.
    pub home_project: Option<String>,
}

impl PurgeRetainedTask {
    /// A collision cannot be auto-deleted because the short task id may name
    /// two different tasks. Give the operator the safe, explicit reassignment
    /// path after they confirm the local `(id, title)` owner.
    fn operator_command_with_endpoint(&self, endpoint: &str) -> Option<String> {
        if self.reason.starts_with("id collision") {
            return Some(format!(
                "cas task show id={}; then mcp__cs__task action=update id={} origin_project=<confirmed canonical id>",
                self.id, self.id
            ));
        }
        if self.reason.starts_with("accepted proposal") {
            return None;
        }
        let home_project = self.home_project.as_deref()?.trim();
        if home_project.is_empty() {
            return None;
        }
        Some(remote_task_delete_command(
            endpoint,
            &self.id,
            &self.title,
            home_project,
        ))
    }
}

/// Render a deliberately manual remote deletion command for a task that the
/// local purge cannot safely classify. The operator must confirm the exact
/// `(id, title)` in the task's home project before running it.
pub(crate) fn remote_task_delete_command(
    endpoint: &str,
    id: &str,
    title: &str,
    home_project: &str,
) -> String {
    let home_project = home_project.trim();
    let encoded_id = urlencoding::encode(id);
    let encoded_project = urlencoding::encode(home_project);
    format!(
        "confirm (id={id}, title={title:?}) in home project `{home_project}`, then run: curl -fsS -X DELETE '{}/api/sync/task/{encoded_id}?project_id={encoded_project}' -H 'Authorization: Bearer $CAS_TOKEN'",
        endpoint.trim_end_matches('/')
    )
}

/// The shared result consumed by purge and doctor's cross-project check. The
/// report count is the doctor's peer/evidence count; the delete set is the
/// concrete, safety-filtered set purge can actually reach.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeForeignAnalysis {
    pub delete_set: PurgeDeleteSet,
    pub foreign_task_count: usize,
    pub retained_foreign_tasks: Vec<PurgeRetainedTask>,
    pub unattributed_task_count: usize,
    pub collision_count: usize,
}

/// Canonical attribution fields that have existed in cloud payloads. The local
/// content stores do not currently persist one for entries, rules, or skills,
/// so an absent field means that kind is not an eligible purge candidate.
const PURGE_PROJECT_COLUMNS: &[&str] = &[
    "origin_project",
    "project_canonical_id",
    "origin_project_id",
    "project_id",
];

fn project_ids_match(candidate: &str, current: &str) -> bool {
    crate::cloud::project_ids_match(candidate, current)
}

fn first_existing_project_column(
    conn: &rusqlite::Connection,
    table: &str,
    preferred: &[&'static str],
) -> anyhow::Result<Option<&'static str>> {
    let mut stmt = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = stmt.query_map([], |row| row.get::<_, String>(1))?;
    let existing: BTreeSet<String> = columns.collect::<Result<_, _>>()?;
    Ok(preferred
        .iter()
        .copied()
        .find(|column| existing.contains(*column)))
}

/// Read ids + labels only when a row explicitly carries a project attribution
/// different from the current canonical id. Missing tables or attribution
/// columns are treated as empty: legacy rows are not safe to call foreign.
fn attributed_rows(
    conn: &rusqlite::Connection,
    kind: &'static str,
    table: &str,
    label_sql: &str,
    current_project: &str,
) -> anyhow::Result<Vec<PurgeEntity>> {
    let Some(project_column) = first_existing_project_column(
        conn,
        table,
        if table == "tasks" {
            &[
                "origin_project",
                "project_canonical_id",
                "origin_project_id",
                "project_id",
            ]
        } else {
            PURGE_PROJECT_COLUMNS
        },
    )?
    else {
        return Ok(Vec::new());
    };

    let sql = format!(
        "SELECT id, {label_sql}, {project_column} FROM {table}
         WHERE NULLIF(trim({project_column}), '') IS NOT NULL
         ORDER BY id"
    );
    let mut stmt = conn.prepare(&sql)?;
    let mapped = stmt.query_map([], |row| {
        Ok((
            (
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            ),
            row.get::<_, String>(2)?,
        ))
    })?;
    let rows = mapped.collect::<Result<Vec<_>, _>>()?;
    Ok(rows
        .into_iter()
        .filter(|(_, stored_project)| !project_ids_match(stored_project, current_project))
        .map(|((id, label), stored_project)| {
            PurgeEntity::with_evidence(kind, id, label, "origin_project", stored_project)
        })
        .collect())
}

const PROPOSAL_PROVENANCE_BEGIN: &str = "--- BEGIN SERVER-ATTESTED PROPOSAL PROVENANCE ---";
const PROPOSAL_PROVENANCE_END: &str = "--- END CLIENT-ASSERTED PROPOSAL PROVENANCE ---";
const PROPOSAL_PROVENANCE_SERVER_END: &str = "--- END SERVER-ATTESTED PROPOSAL PROVENANCE ---";

/// A task materialized by an accepted cross-project proposal is intentionally
/// retained in the target project even though its origin belongs to a peer.
/// Pull persists the server-attested target project in the task notes; only a
/// complete marker block targeting this project qualifies.
fn is_accepted_proposal_task(
    conn: &rusqlite::Connection,
    task_id: &str,
    current_project: &str,
) -> anyhow::Result<bool> {
    let has_notes = conn
        .prepare("PRAGMA table_info(tasks)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .iter()
        .any(|column| column == "notes");
    if !has_notes {
        return Ok(false);
    }

    let notes: Option<String> =
        conn.query_row("SELECT notes FROM tasks WHERE id = ?1", [task_id], |row| {
            row.get(0)
        })?;
    let Some(notes) = notes else {
        return Ok(false);
    };
    let Some(start) = notes.find(PROPOSAL_PROVENANCE_BEGIN) else {
        return Ok(false);
    };
    let Some((end_offset, end_marker)) = [PROPOSAL_PROVENANCE_END, PROPOSAL_PROVENANCE_SERVER_END]
        .iter()
        .filter_map(|marker| notes[start..].find(marker).map(|offset| (offset, *marker)))
        .min_by_key(|(offset, _)| *offset)
    else {
        return Ok(false);
    };
    let end = start + end_offset + end_marker.len();
    let block = &notes[start..end];
    let target = block.lines().find_map(|line| {
        line.trim()
            .strip_prefix("target_project_canonical_id:")
            .map(str::trim)
            .filter(|value| !value.is_empty())
    });
    Ok(target.is_some_and(|target| project_ids_match(target, current_project)))
}

/// A task with a local external-blocker projection is deliberately retained:
/// `blocks_origin_task_id` created this origin-side row and the projection is
/// the local model for that cross-project handoff.
fn has_external_task_dependency(
    conn: &rusqlite::Connection,
    task_id: &str,
) -> anyhow::Result<bool> {
    let result = conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM sqlite_master
             WHERE type = 'table' AND name = 'external_task_dependencies'
         )",
        [],
        |row| row.get::<_, bool>(0),
    )?;
    if !result {
        return Ok(false);
    }
    Ok(conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM external_task_dependencies
             WHERE origin_task_id = ?1
         )",
        [task_id],
        |row| row.get(0),
    )?)
}

fn count_proven_attributed_rules(
    conn: &rusqlite::Connection,
    current_project: &str,
) -> anyhow::Result<usize> {
    let Some(project_column) = first_existing_project_column(conn, "rules", PURGE_PROJECT_COLUMNS)?
    else {
        return Ok(0);
    };
    if first_existing_project_column(conn, "rules", &["status"])?.is_none() {
        return Ok(0);
    }
    let sql = format!(
        "SELECT {project_column} FROM rules
         WHERE lower(status) = 'proven'
           AND NULLIF(trim({project_column}), '') IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
    let mut count = 0;
    for row in rows {
        if !project_ids_match(&row?, current_project) {
            count += 1;
        }
    }
    Ok(count)
}

fn count_purge_dependencies(
    conn: &rusqlite::Connection,
    foreign_tasks: &[PurgeEntity],
) -> anyhow::Result<usize> {
    let foreign_ids: BTreeSet<&str> = foreign_tasks.iter().map(|task| task.id.as_str()).collect();
    if foreign_ids.is_empty() {
        return Ok(0);
    }

    let mut stmt = match conn.prepare("SELECT from_id, to_id FROM dependencies") {
        Ok(stmt) => stmt,
        Err(error) if error.to_string().contains("no such table") => return Ok(0),
        Err(error) => return Err(error.into()),
    };
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut count = 0;
    for row in rows {
        let (from_id, to_id) = row?;
        if foreign_ids.contains(from_id.as_str()) || foreign_ids.contains(to_id.as_str()) {
            count += 1;
        }
    }
    Ok(count)
}

/// Read the ids + labels of every row a purge would delete.
///
/// Missing tables are treated as empty rather than fatal: a database that
/// predates one of these stores must still be previewable. Rows without an
/// explicit project attribution remain local legacy data and are retained.
fn collect_purge_delete_set(
    conn: &rusqlite::Connection,
    current_project: &str,
) -> anyhow::Result<PurgeDeleteSet> {
    let mut tasks = Vec::new();
    for task in attributed_rows(conn, "task", "tasks", "title", current_project)? {
        // Fail closed if a safety lookup cannot be completed. The caller gets
        // the database error rather than a misleadingly smaller delete set.
        if !is_accepted_proposal_task(conn, &task.id, current_project)?
            && !has_external_task_dependency(conn, &task.id)?
        {
            tasks.push(task);
        }
    }
    Ok(PurgeDeleteSet {
        entries: attributed_rows(
            conn,
            "entry",
            "entries",
            "COALESCE(NULLIF(title, ''), substr(content, 1, 80))",
            current_project,
        )?,
        tasks: tasks.clone(),
        rules: attributed_rows(
            conn,
            "rule",
            "rules",
            "substr(content, 1, 80)",
            current_project,
        )?,
        skills: attributed_rows(conn, "skill", "skills", "name", current_project)?,
        dependencies: count_purge_dependencies(conn, &tasks)?,
    })
}

/// Combine the explicit project-column classifier with doctor's read-only
/// cross-database activity evidence. Peer evidence may only expand the task
/// set for rows whose persisted `origin_project` says this is the current
/// project; missing provenance, id collisions, and unattributed replicas are
/// never promoted to deletions.
pub(crate) fn collect_purge_delete_set_with_report(
    conn: &rusqlite::Connection,
    current_project: &str,
    report: &crate::cli::foreign_rows::ForeignRowReport,
) -> anyhow::Result<PurgeForeignAnalysis> {
    let mut delete_set = collect_purge_delete_set(conn, current_project)?;
    let collision_ids = report
        .collisions
        .iter()
        .map(|collision| collision.id.as_str())
        .collect::<BTreeSet<_>>();
    let unattributed_ids = report
        .unattributed
        .iter()
        .map(|row| row.id.as_str())
        .collect::<BTreeSet<_>>();
    // An explicit origin value cannot override the doctor's safety verdict for
    // a collision or an unattributed replica. Both categories are retained.
    delete_set.tasks.retain(|task| {
        !collision_ids.contains(task.id.as_str()) && !unattributed_ids.contains(task.id.as_str())
    });
    let mut known_task_ids = delete_set
        .tasks
        .iter()
        .map(|task| task.id.clone())
        .collect::<BTreeSet<_>>();

    for foreign in &report.foreign {
        if known_task_ids.contains(&foreign.id) {
            continue;
        }
        if collision_ids.contains(foreign.id.as_str())
            || unattributed_ids.contains(foreign.id.as_str())
        {
            continue;
        }
        let origin_is_current = foreign
            .origin_project
            .as_deref()
            .is_some_and(|origin| project_ids_match(origin, current_project));
        if !origin_is_current {
            continue;
        }
        if is_accepted_proposal_task(conn, &foreign.id, current_project)?
            || has_external_task_dependency(conn, &foreign.id)?
        {
            continue;
        }
        delete_set.tasks.push(PurgeEntity::with_evidence(
            "task",
            foreign.id.clone(),
            foreign.title.clone(),
            "peer-evidence",
            foreign.home_project.clone(),
        ));
        known_task_ids.insert(foreign.id.clone());
    }
    delete_set
        .tasks
        .sort_by(|left, right| left.id.cmp(&right.id));
    delete_set.dependencies = count_purge_dependencies(conn, &delete_set.tasks)?;

    let mut retained_foreign_tasks = Vec::new();
    for foreign in &report.foreign {
        if delete_set
            .tasks
            .iter()
            .any(|task| task.id == foreign.id && task.label.trim() == foreign.title.trim())
        {
            continue;
        }
        let reason = if collision_ids.contains(foreign.id.as_str()) {
            "id collision across peer rows; purge fails closed".to_string()
        } else if unattributed_ids.contains(foreign.id.as_str()) {
            "unattributed replica; purge fails closed".to_string()
        } else if is_accepted_proposal_task(conn, &foreign.id, current_project)? {
            "accepted proposal materialized for this project".to_string()
        } else if has_external_task_dependency(conn, &foreign.id)? {
            "blocks_origin external dependency is retained locally".to_string()
        } else if foreign.origin_project.is_none() {
            "origin_project is missing; peer evidence is advisory and purge fails closed"
                .to_string()
        } else {
            "row is not explicitly attributed to the current project".to_string()
        };
        retained_foreign_tasks.push(PurgeRetainedTask {
            id: foreign.id.clone(),
            title: foreign.title.clone(),
            reason,
            home_project: Some(foreign.home_project.clone()),
        });
    }

    Ok(PurgeForeignAnalysis {
        delete_set,
        foreign_task_count: report.foreign.len(),
        retained_foreign_tasks,
        unattributed_task_count: report.unattributed.len(),
        collision_count: report.collisions.len(),
    })
}

/// Build the same purge analysis used by the destructive command from an
/// already-collected doctor report. The database remains read-only.
pub fn purge_analysis_for_report(
    cas_root: &Path,
    current_project: &str,
    report: &crate::cli::foreign_rows::ForeignRowReport,
) -> anyhow::Result<PurgeForeignAnalysis> {
    let db_path = cas_root.join("cas.db");
    let conn = rusqlite::Connection::open_with_flags(
        db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    collect_purge_delete_set_with_report(&conn, current_project, report)
}

/// A named reason the purge refuses to run destructively.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PurgeRefusal {
    /// No successful cloud pull is recorded at all.
    NeverPulled,
    /// The last successful pull is older than the configured threshold.
    StalePull {
        last_pull_at: String,
        age_days: i64,
        threshold_days: i64,
    },
    /// `last_pull_at` exists but cannot be parsed — treated as unknown, and
    /// unknown is not safe.
    UnreadablePullTimestamp { last_pull_at: String },
    /// Local rows are still queued for push: deleting them loses work the
    /// cloud has never seen, and the re-pull cannot restore it.
    UnpushedRows { pending: usize, sample: Vec<String> },
    /// More than half of all local tasks would be removed. This is a hard
    /// stop because a project-scoped classifier has likely lost attribution.
    TooManyForeignTasks { foreign: usize, total: usize },
    /// A proven rule would be removed. Proven rules are never force-deletable.
    ProvenRule { count: usize },
}

impl PurgeRefusal {
    /// Single-sentence, self-explaining reason (AC2: "the refusal names the reason").
    pub fn reason(&self) -> String {
        match self {
            PurgeRefusal::NeverPulled => {
                "no successful cloud pull recorded (last_pull_at is unset) \
— a purge would delete local rows and re-pull from an unverified baseline"
                    .to_string()
            }
            PurgeRefusal::StalePull {
                last_pull_at,
                age_days,
                threshold_days,
            } => format!(
                "stale cloud sync: last successful pull was {age_days} days ago ({last_pull_at}), \
threshold is {threshold_days} days — the re-pull would restore an out-of-date snapshot over \
everything deleted since"
            ),
            PurgeRefusal::UnreadablePullTimestamp { last_pull_at } => format!(
                "cannot read last_pull_at ({last_pull_at:?}); sync freshness is unknown and \
unknown is not safe for a destructive purge"
            ),
            PurgeRefusal::UnpushedRows { pending, sample } => format!(
                "{pending} local change(s) are still queued for push and have never reached the \
cloud (e.g. {}) — the purge would delete them and the re-pull cannot bring them back",
                sample.join(", ")
            ),
            PurgeRefusal::TooManyForeignTasks { foreign, total } => format!(
                "refusing to purge foreign tasks: {foreign} of {total} local tasks exceed the 50% \
safety limit — review project attribution before deleting anything"
            ),
            PurgeRefusal::ProvenRule { count } => format!(
                "refusing to purge {count} proven rule{} — proven rules are protected even with --force",
                if *count == 1 { "" } else { "s" }
            ),
        }
    }

    /// Stable machine-readable code for --json consumers.
    pub fn code(&self) -> &'static str {
        match self {
            PurgeRefusal::NeverPulled => "never_pulled",
            PurgeRefusal::StalePull { .. } => "stale_pull",
            PurgeRefusal::UnreadablePullTimestamp { .. } => "unreadable_pull_timestamp",
            PurgeRefusal::UnpushedRows { .. } => "unpushed_rows",
            PurgeRefusal::TooManyForeignTasks { .. } => "too_many_foreign_tasks",
            PurgeRefusal::ProvenRule { .. } => "proven_rule",
        }
    }

    fn is_hard(&self) -> bool {
        matches!(
            self,
            PurgeRefusal::TooManyForeignTasks { .. } | PurgeRefusal::ProvenRule { .. }
        )
    }
}

/// Safety limits that are intrinsic to the classifier and cannot be bypassed
/// by `--force`. `proven_rule_count` is computed from the same attribution
/// predicate as the delete set, so a proven local rule never trips this guard.
fn evaluate_purge_hard_guards(
    delete_set: &PurgeDeleteSet,
    total_tasks: usize,
    proven_rule_count: usize,
) -> Vec<PurgeRefusal> {
    evaluate_purge_hard_guards_with_options(delete_set, total_tasks, proven_rule_count, false)
}

fn evaluate_purge_hard_guards_with_options(
    delete_set: &PurgeDeleteSet,
    total_tasks: usize,
    proven_rule_count: usize,
    allow_majority_foreign: bool,
) -> Vec<PurgeRefusal> {
    let mut refusals = Vec::new();
    if !allow_majority_foreign
        && total_tasks > 0
        && delete_set.tasks.len().saturating_mul(2) > total_tasks
    {
        refusals.push(PurgeRefusal::TooManyForeignTasks {
            foreign: delete_set.tasks.len(),
            total: total_tasks,
        });
    }
    if proven_rule_count > 0 {
        refusals.push(PurgeRefusal::ProvenRule {
            count: proven_rule_count,
        });
    }
    refusals
}

fn validate_majority_foreign_override(
    allow_majority_foreign: bool,
    yes: bool,
) -> anyhow::Result<()> {
    if allow_majority_foreign && !yes {
        anyhow::bail!(
            "--allow-majority-foreign requires --yes; review a fresh --dry-run before applying"
        );
    }
    Ok(())
}

/// Apply a previously classified delete set atomically. This function accepts
/// only concrete row ids from `PurgeDeleteSet`; it never re-runs an unscoped
/// `DELETE FROM <table>` predicate.
fn delete_purge_rows(
    conn: &mut rusqlite::Connection,
    delete_set: &PurgeDeleteSet,
) -> anyhow::Result<()> {
    let tx = conn.transaction()?;
    for (table, rows) in [
        ("entries", &delete_set.entries),
        ("tasks", &delete_set.tasks),
        ("rules", &delete_set.rules),
        ("skills", &delete_set.skills),
    ] {
        for row in rows {
            let sql = if table == "tasks" {
                format!("DELETE FROM {table} WHERE id = ?1 AND trim(title) = trim(?2)")
            } else {
                format!("DELETE FROM {table} WHERE id = ?1")
            };
            let result = if table == "tasks" {
                tx.execute(&sql, rusqlite::params![&row.id, &row.label])
            } else {
                tx.execute(&sql, [&row.id])
            };
            match result {
                Ok(_) => {}
                Err(error) if error.to_string().contains("no such table") => break,
                Err(error) => return Err(error.into()),
            }
        }
    }

    // Remove only dependency edges attached to a foreign task. Edges between
    // retained local/legacy tasks must survive the purge.
    for task in &delete_set.tasks {
        match tx.execute(
            "DELETE FROM dependencies WHERE from_id = ?1 OR to_id = ?1",
            [&task.id],
        ) {
            Ok(_) => {}
            Err(error) if error.to_string().contains("no such table") => break,
            Err(error) => return Err(error.into()),
        }
    }

    // Reset last_pull_at only when rows were actually removed so a no-op
    // cleanup does not force an unnecessary full pull.
    if delete_set.total() > 0 {
        match tx.execute("DELETE FROM sync_metadata WHERE key = 'last_pull_at'", []) {
            Ok(_) => {}
            Err(error) if error.to_string().contains("no such table") => {}
            Err(error) => return Err(error.into()),
        }
    }
    tx.commit()?;
    Ok(())
}

/// Parse a `last_pull_at` value. Accepts RFC3339 (what the syncer writes) and
/// the naive `%Y-%m-%d %H:%M:%S` shape older rows may carry.
fn parse_sync_timestamp(raw: &str) -> Option<chrono::DateTime<chrono::Utc>> {
    let raw = raw.trim();
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(raw) {
        return Some(dt.with_timezone(&chrono::Utc));
    }
    for fmt in ["%Y-%m-%d %H:%M:%S%.f", "%Y-%m-%dT%H:%M:%S%.f"] {
        if let Ok(naive) = chrono::NaiveDateTime::parse_from_str(raw, fmt) {
            return Some(naive.and_utc());
        }
    }
    None
}

/// Decide whether a destructive purge is safe. Pure — takes the observed state,
/// returns every reason to refuse (empty = safe).
fn evaluate_purge_safety(
    last_pull_at: Option<&str>,
    pending_pushes: &[(String, String)],
    now: chrono::DateTime<chrono::Utc>,
    threshold_days: i64,
) -> Vec<PurgeRefusal> {
    let mut refusals = Vec::new();

    match last_pull_at.map(str::trim).filter(|s| !s.is_empty()) {
        None => refusals.push(PurgeRefusal::NeverPulled),
        Some(raw) => match parse_sync_timestamp(raw) {
            None => refusals.push(PurgeRefusal::UnreadablePullTimestamp {
                last_pull_at: raw.to_string(),
            }),
            Some(ts) => {
                let age_days = (now - ts).num_days();
                if age_days > threshold_days {
                    refusals.push(PurgeRefusal::StalePull {
                        last_pull_at: raw.to_string(),
                        age_days,
                        threshold_days,
                    });
                }
            }
        },
    }

    if !pending_pushes.is_empty() {
        refusals.push(PurgeRefusal::UnpushedRows {
            pending: pending_pushes.len(),
            sample: pending_pushes
                .iter()
                .take(5)
                .map(|(kind, id)| format!("{kind}:{id}"))
                .collect(),
        });
    }

    refusals
}

/// Queued-but-unpushed local changes for the entity kinds a purge deletes.
///
/// Entry access reinforcement is observational metadata written by
/// `SessionStart` through `SyncingEntryStore`; it is not a content mutation
/// that this purge can safely classify from the queue row alone. Tasks, rules,
/// and skills remain guarded because their queued rows represent content or
/// ownership changes.
///
/// Fails CLOSED. Only an absent `sync_queue` table is "no pending pushes";
/// every other read failure (schema drift, corruption, a row that will not
/// decode) is propagated so the purge refuses loudly. Silently returning zero
/// here would disable the unpushed-rows guard in a destructive path — the one
/// place a reassuring wrong answer is most expensive.
pub(crate) fn pending_content_pushes(
    conn: &rusqlite::Connection,
) -> anyhow::Result<Vec<(String, String)>> {
    let mut stmt = match conn.prepare(
        "SELECT entity_type, entity_id FROM sync_queue
         ORDER BY id",
    ) {
        Ok(s) => s,
        // Table absent (older database) — there is genuinely nothing queued.
        Err(e) if e.to_string().contains("no such table") => return Ok(Vec::new()),
        Err(e) => {
            return Err(anyhow::anyhow!(
                "cannot read the sync queue to check for unpushed local changes: {e}"
            ));
        }
    };
    let rows = stmt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .map_err(|e| {
            anyhow::anyhow!("cannot read the sync queue to check for unpushed local changes: {e}")
        })?;

    let mut out = Vec::new();
    for row in rows {
        // No .flatten(): a row that fails to decode must not vanish into a
        // shorter "pending" list that reads as safe.
        let (entity_type, entity_id) = row.map_err(|e| {
            anyhow::anyhow!("unreadable row in the sync queue while checking unpushed changes: {e}")
        })?;
        if matches!(
            entity_type.to_ascii_lowercase().as_str(),
            "task" | "rule" | "skill"
        ) {
            out.push((entity_type, entity_id));
        }
    }
    Ok(out)
}

/// Queued content rows that would survive the purge. A foreign replica may
/// still have a local queue row because an older client pushed it before the
/// origin guard existed; deleting that replica is the cleanup itself, not a
/// loss of user work. Only queue rows outside the concrete delete set keep the
/// unpushed-work refusal active.
pub(crate) fn pending_content_pushes_excluding(
    conn: &rusqlite::Connection,
    delete_set: &PurgeDeleteSet,
) -> anyhow::Result<Vec<(String, String)>> {
    let doomed = delete_set
        .groups()
        .into_iter()
        .flat_map(|(kind, rows)| rows.iter().map(move |row| (kind, row.id.as_str())))
        .map(|(kind, id)| {
            let queue_kind = match kind {
                "entries" => "entry",
                "tasks" => "task",
                "rules" => "rule",
                "skills" => "skill",
                _ => kind,
            };
            (queue_kind.to_string(), id.to_string())
        })
        .collect::<BTreeSet<_>>();

    Ok(pending_content_pushes(conn)?
        .into_iter()
        .filter(|(kind, id)| !doomed.contains(&(kind.to_ascii_lowercase(), id.clone())))
        .collect())
}

/// Crash-safe backup of a live WAL database.
///
/// `std::fs::copy` of `cas.db` silently omits `-wal`/`-shm`, so every committed
/// transaction still sitting in the WAL is missing from the "backup" — the one
/// artifact a destructive purge depends on. `VACUUM INTO` asks SQLite itself to
/// materialise a consistent snapshot (WAL content included) into a new file.
fn backup_database_crash_safe(db_path: &Path, backup_path: &Path) -> anyhow::Result<()> {
    if backup_path.exists() {
        anyhow::bail!(
            "refusing to overwrite existing backup at {}",
            backup_path.display()
        );
    }
    let conn = rusqlite::Connection::open(db_path)?;
    // VACUUM INTO requires the destination not to exist (checked above).
    conn.execute("VACUUM INTO ?1", [backup_path.to_string_lossy().as_ref()])
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to write crash-safe backup to {}: {e}",
                backup_path.display()
            )
        })?;
    Ok(())
}

/// Hash the complete, ordered dry-run payload. The hash binds ids, labels, and
/// classifier evidence, so an apply cannot silently switch to a changed
/// classification between its preview and destructive phase.
fn purge_delete_set_hash(delete_set: &PurgeDeleteSet) -> String {
    format!(
        "sha256:{:x}",
        Sha256::digest(delete_set.to_json().to_string().as_bytes())
    )
}

fn purge_task_ratio(foreign_tasks: usize, total_tasks: usize) -> f64 {
    if total_tasks == 0 {
        0.0
    } else {
        (foreign_tasks as f64 / total_tasks as f64) * 100.0
    }
}

fn verify_purge_delete_set_hash(
    expected: &str,
    fresh_delete_set: &PurgeDeleteSet,
) -> anyhow::Result<()> {
    let actual = purge_delete_set_hash(fresh_delete_set);
    if actual != expected {
        anyhow::bail!(
            "refusing majority-foreign purge: the store changed after the fresh dry-run (delete-set hash {expected} != {actual}); run --dry-run again"
        );
    }
    Ok(())
}

fn inspect_purge_state(
    cas_root: &Path,
    project_id: &str,
    total_tasks: usize,
    stale_days: i64,
    allow_majority_foreign: bool,
) -> anyhow::Result<(PurgeForeignAnalysis, Vec<PurgeRefusal>)> {
    let db_path = cas_root.join("cas.db");
    let conn = rusqlite::Connection::open(&db_path)?;
    // Use doctor's cross-DB classifier so a legacy backfill that stamped
    // foreign rows with this project cannot make purge under-delete them.
    // A failed read is fatal to the purge preview: an incomplete peer scan
    // must not be presented as a complete delete set.
    let doctor_report = crate::cli::foreign_rows::scan(cas_root)?;
    let analysis = collect_purge_delete_set_with_report(&conn, project_id, &doctor_report)?;
    let proven_rule_count = count_proven_attributed_rules(&conn, project_id)?;
    let last_pull_at: Option<String> = conn
        .query_row(
            "SELECT value FROM sync_metadata WHERE key = 'last_pull_at'",
            [],
            |r| r.get(0),
        )
        .ok();
    let pending = pending_content_pushes_excluding(&conn, &analysis.delete_set)?;
    let mut refusals = evaluate_purge_safety(
        last_pull_at.as_deref(),
        &pending,
        chrono::Utc::now(),
        stale_days,
    );
    refusals.extend(evaluate_purge_hard_guards_with_options(
        &analysis.delete_set,
        total_tasks,
        proven_rule_count,
        allow_majority_foreign,
    ));
    Ok((analysis, refusals))
}

fn execute_purge_foreign(
    args: &CloudPurgeForeignArgs,
    cli: &Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    use std::sync::Arc;

    use crate::cloud::{CloudSyncer, CloudSyncerConfig, SyncQueue, resolve_canonical_id_for_sync};

    validate_majority_foreign_override(args.allow_majority_foreign, args.yes)?;

    let config = CloudConfig::load_from_cas_dir_inheriting_user_credentials(cas_root)?;
    if config.token.is_none() {
        anyhow::bail!("Not logged in. Run 'cas login' first");
    }

    let project_id = resolve_canonical_id_for_sync(cas_root)
        .map_err(|error| anyhow::anyhow!(error.to_string()))?;

    // cas-7fbb: delete + re-pull must use local openers so the re-pull does
    // not re-feed SyncQueue.
    let store = open_store_local(cas_root)?;
    let task_store = open_task_store_local(cas_root)?;
    let rule_store = open_rule_store_local(cas_root)?;
    let skill_store = open_skill_store_local(cas_root)?;
    // cas-bba4: extra stores required by the extended `CloudSyncer::pull`
    // signature. purge-foreign only deletes content entities (entries/tasks/
    // rules/skills), so these are passed through purely to satisfy the
    // scoped pull contract — the 5 new entity kinds are repopulated from
    // cloud after the local content wipe.
    let spec_store = open_spec_store(cas_root)?;
    let event_store = open_event_store(cas_root)?;
    let prompt_store = open_prompt_store(cas_root)?;
    let file_change_store = open_file_change_store(cas_root)?;
    let commit_link_store = open_commit_link_store(cas_root)?;

    // Count entities before purge
    let entries_before = store.list().map(|v| v.len()).unwrap_or(0);
    let tasks_before = task_store.list(None).map(|v| v.len()).unwrap_or(0);
    let rules_before = rule_store.list().map(|v| v.len()).unwrap_or(0);
    let skills_before = skill_store.list(None).map(|v| v.len()).unwrap_or(0);
    let total_before = entries_before + tasks_before + rules_before + skills_before;

    let db_path = cas_root.join("cas.db");

    // cas-a034 / GH #132: resolve the concrete delete set and the safety state
    // BEFORE anything destructive happens, so --dry-run can show exactly what
    // would be lost and a real run can refuse when losing it is unrecoverable.
    let (mut analysis, mut refusals) = inspect_purge_state(
        cas_root,
        &project_id,
        tasks_before,
        args.stale_days,
        args.allow_majority_foreign,
    )?;
    let mut delete_set_hash = purge_delete_set_hash(&analysis.delete_set);

    if !args.dry_run && args.allow_majority_foreign {
        // The first inspection is the required fresh dry-run. Recompute the
        // complete state immediately before any backup or delete and bind the
        // destructive operation to the first set's hash. A concurrent edit or
        // classifier change therefore fails closed instead of being purged.
        let fresh_total_tasks = task_store.list(None)?.len();
        let (fresh_analysis, fresh_refusals) = inspect_purge_state(
            cas_root,
            &project_id,
            fresh_total_tasks,
            args.stale_days,
            args.allow_majority_foreign,
        )?;
        if fresh_total_tasks != tasks_before {
            anyhow::bail!(
                "refusing majority-foreign purge: the store task count changed after the fresh dry-run ({} != {}); run --dry-run again",
                tasks_before,
                fresh_total_tasks
            );
        }
        verify_purge_delete_set_hash(&delete_set_hash, &fresh_analysis.delete_set)?;
        analysis = fresh_analysis;
        refusals = fresh_refusals;
        delete_set_hash = purge_delete_set_hash(&analysis.delete_set);
    }

    let delete_set = &analysis.delete_set;

    if cli.json {
        if args.dry_run {
            println!(
                "{}",
                serde_json::json!({
                    "dry_run": true,
                    "project_id": project_id,
                    "entities_before": {
                        "entries": entries_before,
                        "tasks": tasks_before,
                        "rules": rules_before,
                        "skills": skills_before,
                        "total": total_before,
                    },
                    "delete_set": delete_set.to_json(),
                    "delete_set_hash": delete_set_hash,
                    "task_ratio": {
                        "foreign_tasks": delete_set.tasks.len(),
                        "total_tasks": tasks_before,
                        "percent": purge_task_ratio(delete_set.tasks.len(), tasks_before),
                    },
                    "foreign_task_evidence_count": analysis.foreign_task_count,
                    "doctor_exclusions": {
                        "unattributed_tasks": analysis.unattributed_task_count,
                        "id_collisions": analysis.collision_count,
                        "retained_foreign_tasks": analysis.retained_foreign_tasks.len(),
                    },
                    "retained_foreign_tasks": analysis
                        .retained_foreign_tasks
                        .iter()
                        .map(|row| serde_json::json!({
                            "id": row.id,
                            "title": row.title,
                            "reason": row.reason,
                            "operator_command": row.operator_command_with_endpoint(&config.endpoint),
                        }))
                        .collect::<Vec<_>>(),
                    "refusals": refusals.iter()
                        .map(|r| serde_json::json!({"code": r.code(), "reason": r.reason()}))
                        .collect::<Vec<_>>(),
                "would_refuse": refusals.iter().any(|refusal| {
                        refusal.is_hard() || !args.force
                    }),
                })
            );
            return Ok(());
        }
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.newline()?;
        fmt.write_accent("  Purge Foreign Entities")?;
        fmt.newline()?;
        fmt.newline()?;
        fmt.write_muted("  Project: ")?;
        fmt.write_raw(&project_id)?;
        fmt.newline()?;
        fmt.write_muted("  Before:  ")?;
        fmt.write_raw(&format!(
            "{} entries, {} tasks, {} rules, {} skills ({} total)",
            entries_before, tasks_before, rules_before, skills_before, total_before,
        ))?;
        fmt.newline()?;

        if args.dry_run {
            // AC1: show the concrete delete set, not just before-counts.
            fmt.newline()?;
            fmt.write_muted("  Would delete:")?;
            fmt.newline()?;
            for (label, rows) in delete_set.groups() {
                fmt.write_raw(&format!("    {} {}", rows.len(), label))?;
                fmt.newline()?;
                for row in rows.iter().take(PURGE_DRY_RUN_PRINT_LIMIT) {
                    fmt.write_muted("      - ")?;
                    fmt.write_raw(&format!(
                        "{}  {} [{}]",
                        row.id,
                        row.label,
                        row.evidence_label()
                    ))?;
                    fmt.newline()?;
                }
                if rows.len() > PURGE_DRY_RUN_PRINT_LIMIT {
                    fmt.write_muted(&format!(
                        "      … {} more (use --json for the full set)",
                        rows.len() - PURGE_DRY_RUN_PRINT_LIMIT
                    ))?;
                    fmt.newline()?;
                }
            }
            if analysis.foreign_task_count != delete_set.tasks.len() {
                fmt.write_muted(&format!(
                    "    doctor evidence: {} foreign task row(s); purge delete set: {} task row(s)",
                    analysis.foreign_task_count,
                    delete_set.tasks.len()
                ))?;
                fmt.newline()?;
            }
            if analysis.unattributed_task_count > 0 || analysis.collision_count > 0 {
                fmt.write_muted(&format!(
                    "    doctor exclusions: {} unattributed task row(s), {} id collision(s) (never deleted)",
                    analysis.unattributed_task_count,
                    analysis.collision_count
                ))?;
                fmt.newline()?;
                for retained in &analysis.retained_foreign_tasks {
                    let Some(operator_command) =
                        retained.operator_command_with_endpoint(&config.endpoint)
                    else {
                        continue;
                    };
                    fmt.write_muted("      - ")?;
                    fmt.write_raw(&format!(
                        "{}  {} [{}]",
                        retained.id, retained.title, retained.reason
                    ))?;
                    fmt.newline()?;
                    fmt.write_raw(&format!("        operator: {operator_command}"))?;
                    fmt.newline()?;
                }
            }
            fmt.write_muted("    task ratio: ")?;
            fmt.write_raw(&format!(
                "{}/{} ({:.1}%)",
                delete_set.tasks.len(),
                tasks_before,
                purge_task_ratio(delete_set.tasks.len(), tasks_before)
            ))?;
            fmt.newline()?;
            fmt.write_muted("    delete-set hash: ")?;
            fmt.write_raw(&delete_set_hash)?;
            fmt.newline()?;
            fmt.write_raw(&format!("    {} dependency edges", delete_set.dependencies))?;
            fmt.newline()?;

            if !refusals.is_empty() {
                fmt.newline()?;
                let warning_color = fmt.theme().palette.status_warning;
                fmt.write_colored("  \u{26A0} ", warning_color)?;
                fmt.write_raw("A real run would REFUSE:")?;
                fmt.newline()?;
                for refusal in &refusals {
                    fmt.write_muted("    - ")?;
                    fmt.write_raw(&refusal.reason())?;
                    fmt.newline()?;
                }
            }

            fmt.newline()?;
            fmt.write_muted("  (dry run — no changes made)")?;
            fmt.newline()?;
            fmt.write_raw("  Run without --dry-run to purge and re-pull.")?;
            fmt.newline()?;
            return Ok(());
        }
    }

    // AC2: refuse destructive runs whose loss would be unrecoverable, naming
    // the reason. --force is the explicit, documented override.
    if !refusals.is_empty() {
        let reasons = refusals
            .iter()
            .map(|r| format!("  - {}", r.reason()))
            .collect::<Vec<_>>()
            .join("\n");
        if refusals.iter().any(PurgeRefusal::is_hard) {
            anyhow::bail!(
                "Refusing to purge {} local rows:\n{reasons}\n\
These classifier safety limits cannot be overridden with --force.",
                delete_set.total()
            );
        }
        if !args.force {
            anyhow::bail!(
                "Refusing to purge {} local rows:\n{reasons}\n\
Re-run 'cas cloud pull' first, or pass --force to purge anyway (destructive).",
                delete_set.total()
            );
        }
        eprintln!("warning: purging despite safety refusal (--force):\n{reasons}");
    }

    // Step 1: Back up the database (AC3: crash-safe — never fs::copy a live WAL DB)
    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
    let backup_path = cas_root.join(format!("cas.db.pre-purge-{timestamp}"));
    if db_path.exists() {
        backup_database_crash_safe(&db_path, &backup_path)?;
    }

    // Step 2: Delete only the classified foreign rows via direct SQL. The
    // classifier is deliberately resolved before the backup, and this phase
    // consumes its concrete ids rather than repeating a broader predicate.
    // (Preserves: sync_queue, sync_metadata, agents, sessions, verifications,
    // events, prompts, file_changes, commit_links, worktrees and local rows.)
    {
        let mut conn = rusqlite::Connection::open(&db_path)?;
        delete_purge_rows(&mut conn, &delete_set)?;
    }

    // Step 3: Re-pull from cloud with project-scoped filtering
    let queue = SyncQueue::open(cas_root)?;
    queue.init()?;
    // Purge removes the local evidence used by team-pull watermarks. Clear
    // every scoped watermark so the next team pull cannot skip the rows that
    // need to be re-evaluated under the same ownership rule as doctor.
    let cleared_team_watermarks = queue.delete_metadata_with_prefix("last_team_pull_at_")?;
    let syncer = CloudSyncer::new_for_project(
        Arc::new(queue),
        config,
        CloudSyncerConfig::default(),
        project_id.clone(),
        cas_root,
    );

    let pull_result = syncer.pull(
        store.as_ref(),
        task_store.as_ref(),
        rule_store.as_ref(),
        skill_store.as_ref(),
        spec_store.as_ref(),
        event_store.as_ref(),
        prompt_store.as_ref(),
        file_change_store.as_ref(),
        commit_link_store.as_ref(),
    )?;

    // Count entities after re-pull
    let entries_after = store.list().map(|v| v.len()).unwrap_or(0);
    let tasks_after = task_store.list(None).map(|v| v.len()).unwrap_or(0);
    let rules_after = rule_store.list().map(|v| v.len()).unwrap_or(0);
    let skills_after = skill_store.list(None).map(|v| v.len()).unwrap_or(0);
    let total_after = entries_after + tasks_after + rules_after + skills_after;

    let purged = total_before.saturating_sub(total_after);

    if cli.json {
        println!(
            "{}",
            serde_json::json!({
                "project_id": project_id,
                "backup": backup_path,
                "delete_set_hash": delete_set_hash,
                "task_ratio": {
                    "foreign_tasks": delete_set.tasks.len(),
                    "total_tasks": tasks_before,
                    "percent": purge_task_ratio(delete_set.tasks.len(), tasks_before),
                },
                "operator_decision": if args.allow_majority_foreign {
                    "allow-majority-foreign (--yes; fresh delete-set hash reverified)"
                } else {
                    "default purge safety guards"
                },
                "fresh_delete_set_hash_verified": args.allow_majority_foreign,
                "entities_before": {
                    "entries": entries_before,
                    "tasks": tasks_before,
                    "rules": rules_before,
                    "skills": skills_before,
                    "total": total_before,
                },
                "entities_after": {
                    "entries": entries_after,
                    "tasks": tasks_after,
                    "rules": rules_after,
                    "skills": skills_after,
                    "total": total_after,
                },
                "purged": purged,
                "team_watermarks_cleared": cleared_team_watermarks,
                "pull_errors": pull_result.errors,
            })
        );
    } else {
        let theme = ActiveTheme::default();
        let mut out = io::stdout();
        let mut fmt = Formatter::stdout(&mut out, theme);
        fmt.write_muted("  After:   ")?;
        fmt.write_raw(&format!(
            "{} entries, {} tasks, {} rules, {} skills ({} total)",
            entries_after, tasks_after, rules_after, skills_after, total_after,
        ))?;
        fmt.newline()?;
        fmt.write_muted("  Purged:  ")?;
        fmt.write_raw(&format!("{} foreign entities removed", purged))?;
        fmt.newline()?;
        fmt.write_muted("  Team pull watermarks cleared: ")?;
        fmt.write_raw(&cleared_team_watermarks.to_string())?;
        fmt.newline()?;
        fmt.write_muted("  Backup:  ")?;
        fmt.write_raw(&backup_path.to_string_lossy())?;
        fmt.newline()?;
        fmt.write_muted("  Task ratio: ")?;
        fmt.write_raw(&format!(
            "{}/{} ({:.1}%)",
            delete_set.tasks.len(),
            tasks_before,
            purge_task_ratio(delete_set.tasks.len(), tasks_before)
        ))?;
        fmt.newline()?;
        fmt.write_muted("  Operator decision: ")?;
        fmt.write_raw(if args.allow_majority_foreign {
            "allow-majority-foreign (--yes; fresh delete-set hash reverified)"
        } else {
            "default purge safety guards"
        })?;
        fmt.newline()?;

        if !pull_result.errors.is_empty() {
            fmt.newline()?;
            let warning_color = fmt.theme().palette.status_warning;
            fmt.write_colored("  \u{26A0} ", warning_color)?;
            fmt.write_raw(&format!("{} pull errors:", pull_result.errors.len()))?;
            fmt.newline()?;
            for err in &pull_result.errors {
                fmt.write_muted("    - ")?;
                fmt.write_raw(err)?;
                fmt.newline()?;
            }
        }

        fmt.newline()?;
        let success_color = fmt.theme().palette.status_success;
        fmt.write_colored("  \u{2713} ", success_color)?;
        fmt.write_raw("Purge complete. Pending local changes in sync queue are preserved.")?;
        fmt.newline()?;
    }

    Ok(())
}

#[cfg(test)]
mod team_cmd_tests {
    use super::*;
    use tempfile::TempDir;
    use wiremock::matchers::{header, method, path, query_param, query_param_is_missing};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[test]
    fn queue_retry_reason_is_available_as_a_targeted_retry_flag() {
        use clap::Parser;

        let args = CloudQueueArgs::try_parse_from([
            "queue",
            "--retry",
            "--retry-reason",
            "project_mismatch",
        ])
        .unwrap();
        assert!(args.retry);
        assert_eq!(args.retry_reason.as_deref(), Some("project_mismatch"));

        let alias =
            CloudQueueArgs::try_parse_from(["queue", "--retry", "--reason", "timeout"]).unwrap();
        assert_eq!(alias.retry_reason.as_deref(), Some("timeout"));
    }

    #[test]
    fn alias_project_command_accepts_adopt_aliases_flag() {
        let cli = crate::cli::try_parse_from_with_wordmark([
            "cas",
            "cloud",
            "project",
            "--adopt-aliases",
        ])
        .expect("the doctor-provided alias adoption command must parse");

        match cli.command {
            Some(crate::cli::Commands::Cloud(CloudCommands::Project(args))) => {
                assert!(args.adopt_aliases);
                assert!(args.command.is_none());
            }
            _ => panic!("unexpected command parsed"),
        }
    }

    #[test]
    fn active_team_backlog_is_reported_with_sync_command() {
        let root = TempDir::new().unwrap();
        let queue = SyncQueue::open(root.path()).unwrap();
        queue.init().unwrap();
        queue
            .enqueue_for_team(
                crate::cloud::EntityType::Entry,
                "team-entry",
                crate::cloud::SyncOperation::Upsert,
                Some(r#"{"id":"team-entry"}"#),
                "team-42",
            )
            .unwrap();
        queue
            .enqueue_for_team(
                crate::cloud::EntityType::Task,
                "team-task",
                crate::cloud::SyncOperation::Upsert,
                Some(r#"{"id":"team-task"}"#),
                "team-42",
            )
            .unwrap();

        let config = CloudConfig {
            team_id: Some("team-42".to_string()),
            ..Default::default()
        };
        let backlog = active_team_backlog(&queue, &config).unwrap().unwrap();

        assert_eq!(backlog.team_id, "team-42");
        assert_eq!(backlog.pending, 2);
        assert_eq!(backlog.failed, 0);
        assert_eq!(backlog.command, "cas cloud sync");
        assert_eq!(backlog.total(), 2);
    }

    #[test]
    fn local_unlink_removes_only_the_project_cloud_link() {
        let project = TempDir::new().unwrap();
        let cas_root = project.path().join(".cas");
        std::fs::create_dir_all(&cas_root).unwrap();
        let config = CloudConfig::default();
        config.save_to_cas_dir(&cas_root).unwrap();
        let db_path = cas_root.join("cas.db");
        std::fs::write(&db_path, b"local database bytes").unwrap();

        let cli = cli_json();
        execute_unlink(
            &CloudUnlinkArgs {
                purge_remote: false,
                dry_run: false,
            },
            &cli,
            &cas_root,
        )
        .unwrap();

        assert!(!cas_root.join("cloud.json").exists());
        assert_eq!(std::fs::read(&db_path).unwrap(), b"local database bytes");
    }

    #[test]
    fn unlink_remote_discovery_rejects_unscoped_rows() {
        let mut records = BTreeSet::new();
        let error = collect_unlink_records(
            &mut records,
            &serde_json::json!({
                "entries": [{"id": "entry-1"}],
            }),
            "woodworking",
            UnlinkRemoteScope::Personal,
            &["entries"],
        )
        .unwrap_err();

        assert!(error.to_string().contains("without project_id"));
        assert!(records.is_empty());
    }

    #[tokio::test]
    async fn unlink_remote_discovery_collects_personal_and_team_rows() {
        use std::sync::Arc;

        use crate::cloud::{CloudSyncer, CloudSyncerConfig};

        let server = MockServer::start().await;
        let project_id = "github.com/example/woodworking";
        let team_id = "team-1";
        let row = |id: &str| serde_json::json!({"id": id, "project_id": project_id});

        Mock::given(method("GET"))
            .and(path("/api/sync/".to_owned() + "pull"))
            .and(query_param("types", "entries,tasks,knowledge_pages"))
            .and(query_param_is_missing("team_id"))
            .and(query_param("project_id", project_id))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "entries": [row("personal-entry")],
                "tasks": [row("personal-task")],
                "knowledge_pages": [],
            })))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/sync/".to_owned() + "pull"))
            .and(query_param("types", "entries,tasks,knowledge_pages"))
            .and(query_param("team_id", team_id))
            .and(query_param("project_id", project_id))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "entries": [row("team-entry")],
                "tasks": [row("team-task")],
                "knowledge_pages": [],
            })))
            .mount(&server)
            .await;

        let project = TempDir::new().unwrap();
        let queue = Arc::new(SyncQueue::open(project.path()).unwrap());
        let mut config = CloudConfig::default();
        config.endpoint = server.uri();
        config.token = Some("token".to_string());
        let syncer = CloudSyncer::new_for_project(
            queue,
            config,
            CloudSyncerConfig::default(),
            project_id.to_string(),
            project.path(),
        );
        let records = discover_unlink_remote_records(&syncer, project_id, Some(team_id)).unwrap();

        assert_eq!(records.len(), 4);
        assert!(records.iter().any(|record| {
            record.scope == UnlinkRemoteScope::Personal && record.id == "personal-entry"
        }));
        assert!(records.iter().any(|record| {
            record.scope == UnlinkRemoteScope::Team(team_id.to_string()) && record.id == "team-task"
        }));
    }

    #[test]
    fn sync_summary_renders_zero_pull_as_one_quiet_line() {
        let summary = SyncSummary::pull(&crate::cloud::SyncResult::default(), false);
        let mut tf = crate::ui::components::test_helpers::TestFormatter::plain(80);

        render_sync_summary(&mut tf.fmt(), &summary, false).unwrap();

        assert_eq!(
            tf.output(),
            "[OK] Pull complete · nothing newer · personal only\n"
        );
    }

    #[test]
    fn sync_summary_renders_only_nonzero_pull_kinds() {
        let summary = SyncSummary::pull(
            &crate::cloud::SyncResult {
                pulled_entries: 3,
                pulled_tasks: 12,
                pulled_rules: 1,
                pulled_skills: 0,
                ..Default::default()
            },
            true,
        );
        let mut tf = crate::ui::components::test_helpers::TestFormatter::plain(80);

        render_sync_summary(&mut tf.fmt(), &summary, false).unwrap();

        assert_eq!(
            tf.output(),
            "[OK] Pull complete · 3 entries, 12 tasks, 1 rule · team + personal\n"
        );
    }

    #[test]
    fn sync_summary_merges_team_counts_conflicts_and_healed_edges() {
        let mut summary = SyncSummary::pull(
            &crate::cloud::SyncResult {
                pulled_entries: 3,
                conflicts_resolved: 4,
                healed_task_dependencies_to_cloud: 6,
                ..Default::default()
            },
            true,
        );
        summary.merge_team_pull(&crate::cloud::SyncResult {
            pulled_entries: 2,
            pulled_tasks: 1,
            conflicts_resolved: 8,
            conflicts_resolved_local: 3,
            conflicts_resolved_remote: 5,
            healed_task_dependencies_from_cloud: 10,
            ..Default::default()
        });
        let mut tf = crate::ui::components::test_helpers::TestFormatter::plain(120);

        render_sync_summary(&mut tf.fmt(), &summary, false).unwrap();

        assert_eq!(
            tf.output(),
            "[OK] Pull complete · 3 entries · 12 conflicts resolved · team 2 entries, 1 task · edges 6 pushed, 10 pulled · team + personal\n"
        );
    }

    /// `cas update` has to name what happened to edges: a bare "healed" count
    /// could not distinguish real convergence from the repeat churn cas-cf1f
    /// fixed, and a tombstoned edge that was deliberately NOT pushed is a
    /// different event from one that was.
    #[test]
    fn sync_summary_names_edge_deletes_and_tombstone_skips() {
        let summary = SyncSummary::pull(
            &crate::cloud::SyncResult {
                healed_task_dependencies_to_cloud: 2,
                deleted_task_dependencies: 3,
                skipped_task_dependencies_by_tombstone: 4,
                ..Default::default()
            },
            false,
        );
        let mut tf = crate::ui::components::test_helpers::TestFormatter::plain(120);

        render_sync_summary(&mut tf.fmt(), &summary, false).unwrap();

        assert_eq!(
            tf.output(),
            "[OK] Pull complete · nothing newer · edges 2 pushed, 3 deleted, 4 skipped (tombstoned) · personal only\n"
        );
    }

    #[test]
    fn sync_summary_verbose_renders_counted_conflict_details() {
        let now = chrono::Utc::now();
        let mut result = crate::cloud::SyncResult::default();
        result.record_conflict(crate::cloud::SyncConflict {
            entity_type: "entry".to_string(),
            entity_id: "cas-conflict".to_string(),
            local_updated: now,
            remote_updated: now,
            local_revision: None,
            remote_revision: None,
            resolution: crate::cloud::ConflictResolution::RemoteWins,
            action: crate::cloud::ConflictAction::UseRemote,
        });
        let summary = SyncSummary::pull(&result, false);
        let mut tf = crate::ui::components::test_helpers::TestFormatter::plain(120);

        render_sync_summary(&mut tf.fmt(), &summary, true).unwrap();

        assert!(tf.output().contains("1 conflict(s) resolved"));
        assert!(tf.output().contains("entry cas-conflict"));
    }

    #[test]
    fn sync_summary_renders_successful_push_with_batch_and_pending_counts() {
        let summary = SyncSummary::push(
            &crate::cloud::SyncResult {
                batches_run: 2,
                ..Default::default()
            },
            crate::cloud::PushScope::All,
            None,
        );
        let mut tf = crate::ui::components::test_helpers::TestFormatter::plain(80);

        render_sync_summary(&mut tf.fmt(), &summary, false).unwrap();

        assert_eq!(tf.output(), "[OK] Push complete · 2 batches · 0 pending\n");
    }

    /// cas-f64e x cas-cf1f: one sync carries both the push-side per-row
    /// outcomes and the pull-side dependency-edge outcomes. They are rendered
    /// by different branches of the same function, so a summary built from one
    /// SyncResult must not let either set swallow the other.
    #[test]
    fn push_row_outcomes_and_dependency_edge_outcomes_both_render() {
        let mut backlog = crate::cloud::PushBacklog {
            pending: 0,
            failed: 1,
            ..Default::default()
        };
        backlog
            .rejected_by_reason
            .insert("project_mismatch".to_string(), 1);
        let result = crate::cloud::SyncResult {
            batches_run: 1,
            skipped_lww_acked: 4,
            requeued_after_upgrade: 860,
            remaining_backlog: backlog,
            deleted_task_dependencies: 3,
            skipped_task_dependencies_by_tombstone: 9,
            pulled_task_dependencies: 2,
            ..Default::default()
        };

        let push = SyncSummary::push(&result, crate::cloud::PushScope::All, None);
        let mut tf = crate::ui::components::test_helpers::TestFormatter::plain(400);
        render_sync_summary(&mut tf.fmt(), &push, false).unwrap();
        let push_output = tf.output();
        assert!(
            push_output.contains("4 kept newer by cloud"),
            "{push_output}"
        );
        assert!(
            push_output.contains("1 rejected by cloud (project_mismatch ×1)"),
            "{push_output}"
        );

        let pull = SyncSummary::pull(&result, false);
        let mut tf = crate::ui::components::test_helpers::TestFormatter::plain(400);
        render_sync_summary(&mut tf.fmt(), &pull, false).unwrap();
        let pull_output = tf.output();
        assert!(
            pull_output.contains("edges 3 deleted, 9 skipped (tombstoned)"),
            "{pull_output}"
        );
    }

    /// GH #668: the receipt distinguishes what the cloud kept newer from what
    /// it refused, and names the repair for the leading reason. "N rows failed"
    /// stays reserved for failures the cloud never explained.
    #[test]
    fn sync_summary_separates_lww_skips_from_named_cloud_rejections() {
        let mut backlog = crate::cloud::PushBacklog {
            pending: 4,
            failed: 3,
            ..Default::default()
        };
        backlog
            .rejected_by_reason
            .insert("project_mismatch".to_string(), 2);
        backlog
            .rejected_by_reason
            .insert("revision_conflict".to_string(), 1);
        let summary = SyncSummary::push(
            &crate::cloud::SyncResult {
                batches_run: 1,
                skipped_lww_acked: 7,
                remaining_backlog: backlog,
                ..Default::default()
            },
            crate::cloud::PushScope::All,
            None,
        );
        let mut tf = crate::ui::components::test_helpers::TestFormatter::plain(400);

        render_sync_summary(&mut tf.fmt(), &summary, false).unwrap();

        let output = tf.output();
        assert!(output.contains("7 kept newer by cloud"), "{output}");
        assert!(
            output.contains("3 rejected by cloud (project_mismatch ×2, revision_conflict ×1)"),
            "{output}"
        );
        assert!(output.contains("cas cloud link"), "{output}");
        assert!(
            !output.contains("rows failed"),
            "every terminal row here has a named reason: {output}"
        );
    }

    /// Verbose output spends a line per reason so each refusal carries its own
    /// remediation, and reports the post-upgrade requeue it performed.
    #[test]
    fn verbose_push_summary_lists_each_rejection_reason_with_its_remediation() {
        let mut backlog = crate::cloud::PushBacklog {
            pending: 0,
            failed: 1,
            ..Default::default()
        };
        backlog
            .rejected_by_reason
            .insert("scope_mismatch".to_string(), 1);
        let summary = SyncSummary::push(
            &crate::cloud::SyncResult {
                batches_run: 1,
                skipped_lww_acked: 2,
                requeued_after_upgrade: 705,
                remaining_backlog: backlog,
                ..Default::default()
            },
            crate::cloud::PushScope::All,
            None,
        );
        let mut tf = crate::ui::components::test_helpers::TestFormatter::plain(400);

        render_sync_summary(&mut tf.fmt(), &summary, true).unwrap();

        let output = tf.output();
        assert!(output.contains("kept newer by cloud"), "{output}");
        assert!(
            output.contains("requeued after client upgrade: 705"),
            "{output}"
        );
        assert!(
            output.contains("rejected by cloud: 1 row(s) reason=scope_mismatch"),
            "{output}"
        );
        assert!(output.contains("push it from the owning scope"), "{output}");
    }

    #[test]
    fn sync_summary_groups_identical_push_failures_once_with_retry_hint() {
        let message = "Client version 3.7.7 is below minimum 3.8.0";
        let mut summary = SyncSummary::push(
            &crate::cloud::SyncResult {
                batches_run: 2,
                ..Default::default()
            },
            crate::cloud::PushScope::All,
            None,
        );
        summary.failed = 43;
        summary.failures = (0..43).map(|_| message.to_string()).collect();
        let mut tf = crate::ui::components::test_helpers::TestFormatter::plain(120);

        render_sync_summary(&mut tf.fmt(), &summary, false).unwrap();

        assert_eq!(
            tf.output(),
            "[WARN] Push incomplete · 43 rows failed (Client version 3.7.7 is below minimum 3.8.0) · run cas cloud queue --retry\n"
        );
    }

    #[test]
    fn sync_summary_styled_output_uses_formatter_status_color() {
        let summary = SyncSummary::pull(&crate::cloud::SyncResult::default(), false);
        let mut tf = crate::ui::components::test_helpers::TestFormatter::styled(80);

        render_sync_summary(&mut tf.fmt(), &summary, false).unwrap();

        assert!(tf.output().contains('\x1b'));
        assert_eq!(
            tf.output_plain(),
            "✓ Pull complete · nothing newer · personal only\n"
        );
    }

    #[test]
    fn task_transition_receipt_groups_status_changes_by_project_and_source() {
        let result = crate::cloud::SyncResult {
            task_status_transitions: vec![crate::cloud::TaskStatusTransition {
                task_id: "cas-gh451".to_string(),
                project_id: "gabber-studio".to_string(),
                source: "personal_pull".to_string(),
                from: crate::types::TaskStatus::Closed,
                to: crate::types::TaskStatus::Open,
            }],
            ..Default::default()
        };

        assert_eq!(
            task_transition_summary(&result).as_deref(),
            Some(
                "Task status transitions: 1 task(s) — project=gabber-studio source=personal_pull closed→open (1)"
            )
        );
    }

    #[test]
    fn task_transition_receipt_is_quiet_without_status_changes() {
        assert_eq!(
            task_transition_summary(&crate::cloud::SyncResult::default()),
            None
        );
    }

    #[test]
    fn parse_team_uuid_accepts_canonical() {
        let uuid = parse_team_uuid("550e8400-e29b-41d4-a716-446655440000").unwrap();
        assert_eq!(uuid, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn parse_team_uuid_normalises_uppercase() {
        let uuid = parse_team_uuid("550E8400-E29B-41D4-A716-446655440000").unwrap();
        assert_eq!(uuid, "550e8400-e29b-41d4-a716-446655440000");
    }

    #[test]
    fn parse_team_uuid_rejects_slug() {
        let err = parse_team_uuid("petra-stella").unwrap_err();
        assert!(err.contains("expected a team UUID"));
        assert!(err.contains("petra-stella"));
    }

    #[test]
    fn parse_team_uuid_rejects_empty() {
        assert!(parse_team_uuid("").is_err());
    }

    #[test]
    fn parse_team_uuid_rejects_too_short() {
        assert!(parse_team_uuid("abc-123").is_err());
    }

    #[test]
    fn parse_team_uuid_rejects_no_hyphen_form() {
        // uuid crate would parse this as a simple form; our length gate
        // rejects it so the stored value never drifts from canonical.
        let err = parse_team_uuid("550e8400e29b41d4a716446655440000").unwrap_err();
        assert!(err.contains("expected a team UUID"));
    }

    #[test]
    fn parse_team_uuid_rejects_braced_form() {
        let err = parse_team_uuid("{550e8400-e29b-41d4-a716-446655440000}").unwrap_err();
        assert!(err.contains("expected a team UUID"));
    }

    #[test]
    fn parse_team_uuid_rejects_urn_form() {
        let err = parse_team_uuid("urn:uuid:550e8400-e29b-41d4-a716-446655440000").unwrap_err();
        assert!(err.contains("expected a team UUID"));
    }

    fn make_team_info(id: &str, slug: &str, name: &str) -> TeamInfo {
        TeamInfo {
            id: id.to_string(),
            slug: slug.to_string(),
            name: name.to_string(),
            role: "member".to_string(),
        }
    }

    fn config_with_teams(teams: Vec<TeamInfo>) -> CloudConfig {
        let mut config = CloudConfig::default();
        config.teams = teams;
        config
    }

    fn cli_json() -> Cli {
        Cli {
            json: true,
            full: false,
            verbose: false,
            command: None,
        }
    }

    #[test]
    fn team_set_resolution_uuid_passthrough() {
        let config = config_with_teams(vec![]);
        let target =
            resolve_team_set_target(Some("550e8400-e29b-41d4-a716-446655440000"), &config, true)
                .unwrap();
        assert_eq!(
            target,
            TeamSetTarget::Uuid("550e8400-e29b-41d4-a716-446655440000".to_string())
        );
    }

    #[test]
    fn team_set_resolution_slug_hit() {
        let team = make_team_info(
            "550e8400-e29b-41d4-a716-446655440000",
            "petra-stella",
            "Petra Stella",
        );
        let config = config_with_teams(vec![team.clone()]);
        let target = resolve_team_set_target(Some("petra-stella"), &config, true).unwrap();
        assert_eq!(
            target,
            TeamSetTarget::CachedTeam {
                query: Some("petra-stella".to_string()),
                team,
            }
        );
    }

    #[test]
    fn team_set_resolution_slug_miss_lists_cached_slugs() {
        let config = config_with_teams(vec![make_team_info(
            "550e8400-e29b-41d4-a716-446655440000",
            "petra-stella",
            "Petra Stella",
        )]);
        let err = resolve_team_set_target(Some("missing-team"), &config, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("missing-team"));
        assert!(err.contains("petra-stella"));
        assert!(err.contains("cas cloud login"));
        assert!(!err.contains("Slug resolution is not yet supported"));
    }

    #[test]
    fn team_set_resolution_zero_arg_single_team() {
        let team = make_team_info(
            "550e8400-e29b-41d4-a716-446655440000",
            "petra-stella",
            "Petra Stella",
        );
        let config = config_with_teams(vec![team.clone()]);
        let target = resolve_team_set_target(None, &config, true).unwrap();
        assert_eq!(target, TeamSetTarget::CachedTeam { query: None, team });
    }

    #[test]
    fn team_set_resolution_zero_arg_multi_team_errors_with_options() {
        let config = config_with_teams(vec![
            make_team_info(
                "550e8400-e29b-41d4-a716-446655440000",
                "petra-stella",
                "Petra Stella",
            ),
            make_team_info(
                "650e8400-e29b-41d4-a716-446655440000",
                "ozer-health",
                "Ozer Health",
            ),
        ]);
        let err = resolve_team_set_target(None, &config, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("Multiple cached teams"));
        assert!(err.contains("petra-stella"));
        assert!(err.contains("ozer-health"));
    }

    #[test]
    fn team_set_resolution_empty_cache_errors_with_login_hint() {
        let config = config_with_teams(vec![]);
        let err = resolve_team_set_target(None, &config, true)
            .unwrap_err()
            .to_string();
        assert!(err.contains("No cached team memberships"));
        assert!(err.contains("cas cloud login"));
    }

    #[test]
    fn login_team_selection_activates_the_single_cached_team() {
        let team = make_team_info(
            "550e8400-e29b-41d4-a716-446655440000",
            "petra-stella",
            "Petra Stella",
        );
        let user_config = config_with_teams(vec![team.clone()]);
        let mut project_config = CloudConfig::default();

        let outcome = select_cached_team_after_login(&mut project_config, &user_config);

        assert_eq!(outcome, LoginTeamSelection::Activated(team.clone()));
        assert_eq!(project_config.team_id.as_deref(), Some(team.id.as_str()));
        assert_eq!(
            project_config.team_slug.as_deref(),
            Some(team.slug.as_str())
        );
    }

    #[test]
    fn login_team_selection_keeps_existing_team_for_zero_or_many_memberships() {
        let mut project_config = CloudConfig::default();
        project_config.set_team("existing-team", "existing-slug");

        assert_eq!(
            select_cached_team_after_login(&mut project_config, &config_with_teams(vec![])),
            LoginTeamSelection::NoMembership
        );
        assert_eq!(project_config.team_id.as_deref(), Some("existing-team"));

        let many = config_with_teams(vec![
            make_team_info("team-one", "one", "Team One"),
            make_team_info("team-two", "two", "Team Two"),
        ]);
        assert_eq!(
            select_cached_team_after_login(&mut project_config, &many),
            LoginTeamSelection::MultipleMemberships
        );
        assert_eq!(project_config.team_id.as_deref(), Some("existing-team"));
        assert_eq!(project_config.team_slug.as_deref(), Some("existing-slug"));
    }

    #[test]
    fn team_auto_on_writes_true_and_resolves_effective_team() {
        let _guard = crate::test_support::TestEnvGuard::new();
        let project = TempDir::new().unwrap();
        let user = TempDir::new().unwrap();

        let project_cfg = CloudConfig::default();
        project_cfg.save_to_cas_dir(project.path()).unwrap();

        let mut user_cfg = CloudConfig::default();
        user_cfg.default_team_id = Some("team-1".to_string());
        user_cfg.teams = vec![make_team_info("team-1", "petra-stella", "Petra Stella")];
        user_cfg.save_to_cas_dir(user.path()).unwrap();

        let user_cloud_json = user.path().join("cloud.json");
        unsafe {
            std::env::set_var("CAS_USER_CLOUD_JSON", &user_cloud_json);
        }
        execute_team_auto(&CloudTeamAutoCommands::On, &cli_json(), project.path()).unwrap();
        let saved = CloudConfig::load_from_cas_dir(project.path()).unwrap();
        assert_eq!(saved.team_auto_promote, Some(true));
        assert_eq!(
            saved
                .active_team_id_with_user_config(Some(&user_cfg))
                .as_deref(),
            Some("team-1")
        );
        unsafe {
            std::env::remove_var("CAS_USER_CLOUD_JSON");
        }
    }

    #[test]
    fn team_auto_off_writes_false_kill_switch() {
        let _guard = crate::test_support::TestEnvGuard::new();
        let project = TempDir::new().unwrap();
        let mut project_cfg = CloudConfig::default();
        project_cfg.set_team("team-1", "petra-stella");
        project_cfg.team_auto_promote = Some(true);
        project_cfg.save_to_cas_dir(project.path()).unwrap();

        execute_team_auto(&CloudTeamAutoCommands::Off, &cli_json(), project.path()).unwrap();
        let saved = CloudConfig::load_from_cas_dir(project.path()).unwrap();
        assert_eq!(saved.team_auto_promote, Some(false));
        assert_eq!(saved.active_team_id_with_user_config(None), None);
    }

    #[test]
    fn team_auto_clear_writes_none() {
        let _guard = crate::test_support::TestEnvGuard::new();
        let project = TempDir::new().unwrap();
        let mut project_cfg = CloudConfig::default();
        project_cfg.team_auto_promote = Some(false);
        project_cfg.save_to_cas_dir(project.path()).unwrap();

        execute_team_auto(&CloudTeamAutoCommands::Clear, &cli_json(), project.path()).unwrap();
        let saved = CloudConfig::load_from_cas_dir(project.path()).unwrap();
        assert_eq!(saved.team_auto_promote, None);
    }

    #[test]
    fn find_team_display_prefers_project_slug_for_direct_team_match() {
        let mut project = CloudConfig::default();
        project.set_team("team-1", "project-slug");
        let user = config_with_teams(vec![make_team_info(
            "team-1",
            "user-cache-slug",
            "User Cache Name",
        )]);

        let (slug, name) = find_team_display("team-1", &project, &user);
        assert_eq!(slug, Some("project-slug"));
        assert_eq!(name, None);
    }

    #[test]
    fn config_set_team_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let path = tmp.path().join("cloud.json");

        let mut config = CloudConfig::default();
        config.set_team("550e8400-e29b-41d4-a716-446655440000", "<unknown>");
        config.save_to(&path).unwrap();

        let loaded = CloudConfig::load_from(&path).unwrap();
        assert_eq!(
            loaded.team_id.as_deref(),
            Some("550e8400-e29b-41d4-a716-446655440000")
        );
        assert_eq!(loaded.team_slug.as_deref(), Some("<unknown>"));

        let mut loaded = loaded;
        loaded.clear_team();
        loaded.save_to(&path).unwrap();

        let reloaded = CloudConfig::load_from(&path).unwrap();
        assert!(reloaded.team_id.is_none());
        assert!(reloaded.team_slug.is_none());
    }

    // These probe_membership tests use `tokio::task::spawn_blocking` to call
    // the synchronous `ureq`-based `probe_team_membership` from inside
    // `#[tokio::test]` (which runs on a current-thread runtime). `wiremock`
    // binds the MockServer on the test's tokio runtime; the blocking call
    // executes on tokio's separate blocking pool. `await`-ing the join
    // handle drives the runtime so the mock can serve the request — if you
    // ever replace this pattern, be sure the HTTP call still has a live
    // runtime to answer it on the other side.
    #[tokio::test]
    async fn probe_membership_returns_member_on_200() {
        let server = MockServer::start().await;
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        Mock::given(method("GET"))
            .and(path(format!("/api/teams/{uuid}/projects")))
            .and(header("Authorization", "Bearer test-token"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::json!({ "projects": [] })),
            )
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let outcome = tokio::task::spawn_blocking(move || {
            probe_team_membership(&endpoint, "test-token", uuid)
        })
        .await
        .unwrap();

        assert_eq!(outcome, TeamProbeOutcome::Member);
    }

    #[tokio::test]
    async fn probe_membership_returns_unauthorized_on_401() {
        let server = MockServer::start().await;
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        Mock::given(method("GET"))
            .and(path(format!("/api/teams/{uuid}/projects")))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let outcome = tokio::task::spawn_blocking(move || {
            probe_team_membership(&endpoint, "bad-token", uuid)
        })
        .await
        .unwrap();

        assert_eq!(outcome, TeamProbeOutcome::Unauthorized);
    }

    #[tokio::test]
    async fn probe_membership_returns_not_a_member_on_403() {
        let server = MockServer::start().await;
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        Mock::given(method("GET"))
            .and(path(format!("/api/teams/{uuid}/projects")))
            .respond_with(ResponseTemplate::new(403))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let outcome = tokio::task::spawn_blocking(move || {
            probe_team_membership(&endpoint, "test-token", uuid)
        })
        .await
        .unwrap();

        assert_eq!(outcome, TeamProbeOutcome::NotAMember);
    }

    #[tokio::test]
    async fn probe_membership_returns_not_found_on_404() {
        let server = MockServer::start().await;
        let uuid = "00000000-0000-0000-0000-000000000000";
        Mock::given(method("GET"))
            .and(path(format!("/api/teams/{uuid}/projects")))
            .respond_with(ResponseTemplate::new(404))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let outcome = tokio::task::spawn_blocking(move || {
            probe_team_membership(&endpoint, "test-token", uuid)
        })
        .await
        .unwrap();

        assert_eq!(outcome, TeamProbeOutcome::NotFound);
    }

    #[tokio::test]
    async fn probe_membership_returns_error_on_500() {
        let server = MockServer::start().await;
        let uuid = "550e8400-e29b-41d4-a716-446655440000";
        Mock::given(method("GET"))
            .and(path(format!("/api/teams/{uuid}/projects")))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;
        let endpoint = server.uri();

        let outcome = tokio::task::spawn_blocking(move || {
            probe_team_membership(&endpoint, "test-token", uuid)
        })
        .await
        .unwrap();

        match outcome {
            TeamProbeOutcome::Error(msg) => assert_eq!(msg, "unexpected HTTP 500"),
            other => panic!("expected Error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn probe_membership_returns_error_on_network_failure() {
        // Port 1 is never open on a normal machine; ureq will fail with a
        // transport error. We don't pin the exact wording because ureq's
        // Display differs across platforms, but the prefix is ours.
        let endpoint = "http://127.0.0.1:1".to_string();
        let uuid = "550e8400-e29b-41d4-a716-446655440000";

        let outcome = tokio::task::spawn_blocking(move || {
            probe_team_membership(&endpoint, "test-token", uuid)
        })
        .await
        .unwrap();

        match outcome {
            TeamProbeOutcome::Error(msg) => {
                assert!(
                    msg.starts_with("network error:"),
                    "expected `network error: ...`, got: {msg}"
                );
            }
            other => panic!("expected Error, got {other:?}"),
        }
    }

    // ── check_bucket_ambiguity unit tests (cas-f07a AC1) ──────────────────
    //
    // Pure-function tests — no live cloud, no wiremock. Each creates an
    // in-memory projects slice and asserts the ambiguity detector's output.

    fn make_project(canonical_id: &str, memory_count: u32) -> crate::cloud::TeamProject {
        crate::cloud::TeamProject {
            id: format!("id-{canonical_id}"),
            canonical_id: canonical_id.to_string(),
            name: canonical_id.to_string(),
            contributor_count: 1,
            memory_count,
        }
    }

    #[test]
    fn bucket_ambiguity_warns_when_resolved_is_underpopulated() {
        // Scenario from the ozer bug: git-remote slug has 666 memories while
        // the short-name bucket has 19 285. 666 / 19285 ≈ 3.4 % → warn.
        let projects = vec![
            make_project("github.com/Richards-LLC/ozer-health", 666),
            make_project("ozer", 19_285),
        ];
        let result = check_bucket_ambiguity("github.com/Richards-LLC/ozer-health", &projects);
        let (richer_id, resolved_count, richer_count) =
            result.expect("should warn when resolved bucket is < 10% of richest");
        assert_eq!(richer_id, "ozer");
        assert_eq!(resolved_count, 666);
        assert_eq!(richer_count, 19_285);
    }

    #[test]
    fn bucket_ambiguity_no_warn_when_resolved_is_richest() {
        // The resolved bucket IS the richest — no warning.
        let projects = vec![
            make_project("cas-src", 5_000),
            make_project("github.com/foo/cas-src", 10),
        ];
        assert!(
            check_bucket_ambiguity("cas-src", &projects).is_none(),
            "should not warn when resolved bucket is richest"
        );
    }

    #[test]
    fn bucket_ambiguity_no_warn_when_resolved_not_in_list() {
        // If the resolved canonical_id isn't in the team projects list yet
        // (brand-new project that hasn't synced), skip the warning.
        let projects = vec![make_project("other-project", 10_000)];
        assert!(
            check_bucket_ambiguity("new-project", &projects).is_none(),
            "should not warn when resolved id is absent from project list"
        );
    }

    #[test]
    fn bucket_ambiguity_matches_project_aliases_case_insensitively() {
        let projects = vec![
            make_project("https://GitHub.com/Richards-LLC/gabber-studio.git", 20),
            make_project("other-project", 1_000),
        ];

        let result = check_bucket_ambiguity("gabber-studio", &projects);
        let (richer_id, resolved_count, richer_count) =
            result.expect("remote project alias must match the resolved slug");
        assert_eq!(richer_id, "other-project");
        assert_eq!(resolved_count, 20);
        assert_eq!(richer_count, 1_000);
    }

    #[test]
    fn bucket_ambiguity_no_warn_when_richest_below_threshold() {
        // All projects are small; the 50-memory guard suppresses the warning
        // so new teams don't see noise on early setup.
        let projects = vec![make_project("slug-a", 5), make_project("slug-b", 40)];
        assert!(
            check_bucket_ambiguity("slug-a", &projects).is_none(),
            "should not warn when richest other project is < 50 memories"
        );
    }

    #[test]
    fn bucket_ambiguity_no_warn_at_10_pct_boundary() {
        // resolved = 10, richest = 100. 10/100 = 10%. The check is
        // `resolved * 10 < richest`, i.e. `100 < 100` = false → no warn.
        let projects = vec![
            make_project("slug-small", 10),
            make_project("slug-big", 100),
        ];
        assert!(
            check_bucket_ambiguity("slug-small", &projects).is_none(),
            "should not warn at exactly the 10% boundary"
        );
    }

    #[test]
    fn bucket_ambiguity_warns_just_below_10_pct_boundary() {
        // resolved = 9, richest = 100. 9 * 10 = 90 < 100 → warn.
        let projects = vec![make_project("slug-small", 9), make_project("slug-big", 100)];
        assert!(
            check_bucket_ambiguity("slug-small", &projects).is_some(),
            "should warn when resolved count * 10 < richest count"
        );
    }
}

// T4 team-sync tests now live in `cas-cli/tests/team_sync_test.rs` — an
// integration-test binary that exercises `execute_team_push` end-to-end
// with wiremock. Extracted per the task-verifier feedback on cas-1f44:
// tests are easier to find in the integration tree than buried in this
// 2.4k-line CLI file.

// ═══════════════════════════════════════════════════════════════════════════════
// PURGE-FOREIGN safety tests (cas-a034 / GH #132)
//
// All three ACs are covered without a live cloud: the delete-set reader, the
// staleness/unpushed-rows guard and the crash-safe backup are pure functions
// over a local SQLite file.
// ═══════════════════════════════════════════════════════════════════════════════
#[cfg(test)]
mod purge_foreign_safety_tests {
    use super::*;
    use crate::store::init_cas_dir;
    use crate::types::Task;
    use rusqlite::Connection;
    use tempfile::TempDir;

    fn seed_project_scoped_db(conn: &Connection) {
        conn.execute_batch(
            r#"
            CREATE TABLE entries (
                id TEXT PRIMARY KEY,
                title TEXT,
                content TEXT NOT NULL,
                project_canonical_id TEXT
            );
            CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                origin_project TEXT
            );
            CREATE TABLE rules (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                status TEXT NOT NULL,
                project_canonical_id TEXT
            );
            CREATE TABLE skills (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                project_canonical_id TEXT
            );
            CREATE TABLE dependencies (from_id TEXT, to_id TEXT);

            INSERT INTO tasks (id, title, origin_project) VALUES
                ('own-1', 'own task 1', 'cas-src'),
                ('own-2', 'own task 2', 'cas-src'),
                ('legacy', 'legacy local task', NULL),
                ('foreign-1', 'foreign task 1', 'gabber-studio'),
                ('foreign-2', 'foreign task 2', 'pulse-card');
            INSERT INTO rules (id, content, status, project_canonical_id)
                VALUES ('proven-own', 'keep me', 'proven', 'cas-src');
            INSERT INTO dependencies (from_id, to_id) VALUES
                ('foreign-1', 'own-1'), ('own-2', 'legacy');
            "#,
        )
        .unwrap();
    }

    #[test]
    fn project_scoped_classifier_only_lists_explicitly_foreign_rows() {
        let conn = Connection::open_in_memory().unwrap();
        seed_project_scoped_db(&conn);

        let set = collect_purge_delete_set(&conn, "cas-src").unwrap();

        assert_eq!(
            set.tasks
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            vec!["foreign-1", "foreign-2"]
        );
        assert!(set.entries.is_empty());
        assert!(set.rules.is_empty(), "the proven local rule is protected");
        assert!(set.skills.is_empty());
        assert_eq!(set.dependencies, 1);
        assert!(
            set.tasks.len() <= 5,
            "delete count cannot exceed rows present"
        );
    }

    #[test]
    fn backfilled_current_origin_rows_use_doctor_peer_evidence_and_label_the_source() {
        use crate::cli::foreign_rows::{ForeignRow, ForeignRowReport};

        let conn = Connection::open_in_memory().unwrap();
        seed_project_scoped_db(&conn);
        let report = ForeignRowReport {
            local_project: "cas-src".to_string(),
            local_task_count: 5,
            peers_compared: vec!["accounting".to_string()],
            foreign: vec![ForeignRow {
                id: "own-1".to_string(),
                title: "own task 1".to_string(),
                closed: false,
                origin_project: Some("cas-src".to_string()),
                home_project: "accounting".to_string(),
                also_present_in: Vec::new(),
            }],
            ..Default::default()
        };

        let analysis = collect_purge_delete_set_with_report(&conn, "cas-src", &report).unwrap();
        let row = analysis
            .delete_set
            .tasks
            .iter()
            .find(|row| row.id == "own-1")
            .expect("doctor peer evidence must add the backfilled row");
        assert_eq!(row.evidence.source, "peer-evidence");
        assert_eq!(row.evidence.project, "accounting");
        assert_eq!(
            analysis.delete_set.to_json()["tasks"][0]["evidence"]["source"],
            "origin_project"
        );
        assert_eq!(
            row_to_json(&analysis.delete_set, "own-1")["evidence"]["project"],
            "accounting"
        );
    }

    #[test]
    fn accepted_proposal_tasks_with_foreign_origin_are_never_purge_candidates() {
        let conn = Connection::open_in_memory().unwrap();
        seed_project_scoped_db(&conn);
        conn.execute("ALTER TABLE tasks ADD COLUMN notes TEXT", [])
            .unwrap();
        conn.execute(
            "UPDATE tasks SET notes = ?1 WHERE id = 'foreign-1'",
            ["--- BEGIN SERVER-ATTESTED PROPOSAL PROVENANCE ---\n  target_project_canonical_id: cas-src\n--- END SERVER-ATTESTED PROPOSAL PROVENANCE ---"],
        )
        .unwrap();

        let set = collect_purge_delete_set(&conn, "cas-src").unwrap();

        assert!(
            set.tasks.iter().all(|task| task.id != "foreign-1"),
            "an accepted proposal materialized in this project must be retained"
        );
    }

    #[test]
    fn blocks_origin_and_doctor_safety_categories_are_never_purged() {
        use crate::cli::foreign_rows::{
            ForeignRow, ForeignRowReport, IdCollision, UnattributedRow,
        };

        let conn = Connection::open_in_memory().unwrap();
        seed_project_scoped_db(&conn);
        conn.execute_batch(
            "CREATE TABLE external_task_dependencies (
                 origin_task_id TEXT NOT NULL,
                 target_task_id TEXT NOT NULL
             );
             INSERT INTO external_task_dependencies (origin_task_id, target_task_id)
             VALUES ('own-2', 'remote-1');",
        )
        .unwrap();
        let report = ForeignRowReport {
            local_project: "cas-src".to_string(),
            local_task_count: 5,
            peers_compared: vec!["accounting".to_string()],
            foreign: vec![
                ForeignRow {
                    id: "own-2".to_string(),
                    title: "own task 2".to_string(),
                    closed: false,
                    origin_project: Some("cas-src".to_string()),
                    home_project: "accounting".to_string(),
                    also_present_in: Vec::new(),
                },
                ForeignRow {
                    id: "foreign-1".to_string(),
                    title: "foreign task 1".to_string(),
                    closed: false,
                    origin_project: Some("other-project".to_string()),
                    home_project: "accounting".to_string(),
                    also_present_in: Vec::new(),
                },
                ForeignRow {
                    id: "foreign-2".to_string(),
                    title: "foreign task 2".to_string(),
                    closed: false,
                    origin_project: Some("other-project".to_string()),
                    home_project: "accounting".to_string(),
                    also_present_in: Vec::new(),
                },
            ],
            unattributed: vec![UnattributedRow {
                id: "foreign-1".to_string(),
                title: "foreign task 1".to_string(),
                closed: false,
                present_in: vec!["accounting".to_string()],
            }],
            collisions: vec![IdCollision {
                id: "foreign-2".to_string(),
                local_title: "foreign task 2".to_string(),
                other_project: "accounting".to_string(),
                other_title: "different task".to_string(),
            }],
            ..Default::default()
        };

        let analysis = collect_purge_delete_set_with_report(&conn, "cas-src", &report).unwrap();

        assert!(
            analysis
                .delete_set
                .tasks
                .iter()
                .all(|task| { !matches!(task.id.as_str(), "own-2" | "foreign-1" | "foreign-2") })
        );
        assert_eq!(analysis.unattributed_task_count, 1);
        assert_eq!(analysis.collision_count, 1);
        assert!(
            analysis
                .retained_foreign_tasks
                .iter()
                .any(|task| task.id == "own-2" && task.reason.contains("blocks_origin"))
        );
        assert!(
            analysis
                .retained_foreign_tasks
                .iter()
                .any(|task| task.id == "foreign-2" && task.reason.contains("id collision")),
            "majority override must not turn an id collision into a deletion"
        );
    }

    fn row_to_json(set: &PurgeDeleteSet, id: &str) -> serde_json::Value {
        set.to_json()["tasks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["id"] == id)
            .cloned()
            .unwrap()
    }

    #[test]
    fn alias_rows_are_not_classified_as_foreign() {
        let conn = Connection::open_in_memory().unwrap();
        seed_project_scoped_db(&conn);
        conn.execute(
            "UPDATE tasks SET origin_project = 'gabber-studio' WHERE id IN ('own-1', 'own-2')",
            [],
        )
        .unwrap();
        conn.execute(
            "UPDATE tasks SET origin_project = ?1 WHERE id = 'foreign-1'",
            ["git@GitHub.com:Richards-LLC/gabber-studio.git"],
        )
        .unwrap();

        let set = collect_purge_delete_set(&conn, "gabber-studio").unwrap();

        assert_eq!(
            set.tasks
                .iter()
                .map(|task| task.id.as_str())
                .collect::<Vec<_>>(),
            vec!["foreign-2"]
        );
    }

    #[test]
    fn alias_adoption_rewrites_tasks_and_enqueues_canonical_upserts() {
        let temp = TempDir::new().unwrap();
        let project = temp.path().join("gabber-studio");
        let cas_root = init_cas_dir(&project).unwrap();
        crate::cloud::set_canonical_id_in_config_toml(&cas_root, "gabber-studio").unwrap();

        let task_store = crate::store::open_task_store_local(&cas_root).unwrap();
        let mut alias_task = Task::new("alias-adopt-1".to_string(), "legacy alias".to_string());
        alias_task.origin_project =
            Some("https://GitHub.com/Richards-LLC/gabber-studio.git/".to_string());
        task_store.add(&alias_task).unwrap();

        let mut canonical_task = Task::new(
            "canonical-adopt-1".to_string(),
            "already canonical".to_string(),
        );
        canonical_task.origin_project = Some("gabber-studio".to_string());
        task_store.add(&canonical_task).unwrap();

        let cli = Cli {
            json: true,
            full: false,
            verbose: false,
            command: None,
        };
        execute_project_adopt_aliases(&cli, &cas_root).unwrap();

        assert_eq!(
            task_store
                .get("alias-adopt-1")
                .unwrap()
                .origin_project
                .as_deref(),
            Some("gabber-studio")
        );
        assert_eq!(
            task_store
                .get("canonical-adopt-1")
                .unwrap()
                .origin_project
                .as_deref(),
            Some("gabber-studio")
        );

        let queue = SyncQueue::open(&cas_root).unwrap();
        queue.init().unwrap();
        let queued = queue.pending(10, 5).unwrap();
        assert_eq!(queued.len(), 1, "only the rewritten alias should enqueue");
        assert_eq!(queued[0].entity_id, "alias-adopt-1");
        assert!(
            queued[0]
                .payload
                .as_deref()
                .is_some_and(|payload| payload.contains("\"origin_project\":\"gabber-studio\""))
        );
    }

    #[test]
    fn applying_the_delete_set_keeps_local_rows_and_edges() {
        let mut conn = Connection::open_in_memory().unwrap();
        seed_project_scoped_db(&conn);
        let set = collect_purge_delete_set(&conn, "cas-src").unwrap();

        delete_purge_rows(&mut conn, &set).unwrap();

        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM tasks", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            3,
            "the two foreign tasks are deleted; own and legacy rows remain"
        );
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM dependencies", [], |row| row
                .get::<_, i64>(0),)
                .unwrap(),
            1,
            "the local-to-legacy edge remains"
        );
    }

    #[test]
    fn foreign_task_majority_is_a_hard_purge_refusal() {
        let conn = Connection::open_in_memory().unwrap();
        seed_project_scoped_db(&conn);
        conn.execute(
            "UPDATE tasks SET origin_project = 'other-project' WHERE origin_project IS NULL OR origin_project = 'cas-src'",
            [],
        )
        .unwrap();

        let set = collect_purge_delete_set(&conn, "cas-src").unwrap();
        let refusals = evaluate_purge_hard_guards(&set, 5, 0);

        assert!(refusals.iter().any(|refusal| {
            refusal.code() == "too_many_foreign_tasks" && refusal.reason().contains("5 of 5")
        }));
    }

    #[test]
    fn majority_foreign_override_requires_yes() {
        let error = validate_majority_foreign_override(true, false).unwrap_err();
        assert!(error.to_string().contains("requires --yes"), "{error}");
        validate_majority_foreign_override(true, true).unwrap();
        validate_majority_foreign_override(false, false).unwrap();
    }

    #[test]
    fn majority_foreign_override_lifts_only_the_ratio_guard() {
        let delete_set = PurgeDeleteSet {
            tasks: (0..3)
                .map(|index| {
                    PurgeEntity::with_evidence(
                        "task",
                        format!("foreign-{index}"),
                        "foreign",
                        "origin_project",
                        "other-project",
                    )
                })
                .collect(),
            ..Default::default()
        };

        let refusals = evaluate_purge_hard_guards_with_options(&delete_set, 5, 1, true);

        assert_eq!(
            refusals,
            vec![PurgeRefusal::ProvenRule { count: 1 }],
            "the override removes only the majority ratio refusal"
        );
    }

    #[test]
    fn majority_foreign_apply_rejects_a_stale_dry_run_hash() {
        let conn = Connection::open_in_memory().unwrap();
        seed_db(&conn);
        let first = collect_purge_delete_set(&conn, "test-project").unwrap();
        let expected = purge_delete_set_hash(&first);
        let mut changed = first.clone();
        changed.tasks.push(PurgeEntity::with_evidence(
            "task",
            "new-foreign",
            "new row",
            "origin_project",
            "other-project",
        ));

        let error = verify_purge_delete_set_hash(&expected, &changed).unwrap_err();

        assert!(
            error.to_string().contains("store changed")
                && error.to_string().contains("run --dry-run again"),
            "{error}"
        );
    }

    #[test]
    fn proven_foreign_rule_is_a_hard_purge_refusal() {
        let conn = Connection::open_in_memory().unwrap();
        seed_project_scoped_db(&conn);
        conn.execute(
            "INSERT INTO rules (id, content, status, project_canonical_id)
             VALUES ('proven-foreign', 'do not delete', 'proven', 'other-project')",
            [],
        )
        .unwrap();

        let set = collect_purge_delete_set(&conn, "cas-src").unwrap();
        assert_eq!(set.rules.len(), 1);
        let refusals = evaluate_purge_hard_guards(&set, 5, 1);

        assert!(refusals.iter().any(|refusal| {
            refusal.code() == "proven_rule" && refusal.reason().contains("1 proven rule")
        }));
    }

    /// Minimal shape of the tables purge-foreign touches.
    fn seed_db(conn: &Connection) {
        conn.execute_batch(
            r#"
            CREATE TABLE entries (
                id TEXT PRIMARY KEY,
                title TEXT,
                content TEXT NOT NULL,
                project_canonical_id TEXT
            );
            CREATE TABLE tasks (
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                origin_project TEXT
            );
            CREATE TABLE rules (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL,
                status TEXT NOT NULL DEFAULT 'draft',
                project_canonical_id TEXT
            );
            CREATE TABLE skills (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                project_canonical_id TEXT
            );
            CREATE TABLE dependencies (from_id TEXT, to_id TEXT);
            CREATE TABLE sync_metadata (key TEXT PRIMARY KEY, value TEXT NOT NULL);
            CREATE TABLE sync_queue (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                entity_type TEXT NOT NULL,
                entity_id TEXT NOT NULL,
                operation TEXT NOT NULL
            );

            INSERT INTO entries (id, title, content, project_canonical_id) VALUES
                ('e1', 'Local learning', 'body one', 'other-project'),
                ('e2', NULL, 'untitled body falls back to content', 'other-project');
            INSERT INTO tasks (id, title, origin_project)
                VALUES ('cas-0001', 'Fix the purge guard', 'other-project');
            INSERT INTO rules (id, content, project_canonical_id)
                VALUES ('r1', 'always verify', 'other-project');
            INSERT INTO skills (id, name, project_canonical_id)
                VALUES ('s1', 'release-notes', 'other-project');
            INSERT INTO dependencies (from_id, to_id) VALUES ('cas-0001', 'cas-0002');
            "#,
        )
        .unwrap();
    }

    fn now() -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339("2026-08-07T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    // ── AC1: the dry run has a concrete delete set to print ────────────────────

    #[test]
    fn delete_set_lists_ids_and_titles_for_every_kind() {
        let conn = Connection::open_in_memory().unwrap();
        seed_db(&conn);

        let set = collect_purge_delete_set(&conn, "test-project").unwrap();

        assert_eq!(set.total(), 5, "2 entries + 1 task + 1 rule + 1 skill");
        assert_eq!(set.dependencies, 1);

        assert_eq!(set.entries[0].id, "e1");
        assert_eq!(set.entries[0].label, "Local learning");
        // Untitled entries still get a usable label from their content.
        assert_eq!(set.entries[1].id, "e2");
        assert_eq!(set.entries[1].label, "untitled body falls back to content");

        assert_eq!(set.tasks[0].id, "cas-0001");
        assert_eq!(set.tasks[0].label, "Fix the purge guard");
        assert_eq!(set.rules[0].label, "always verify");
        assert_eq!(set.skills[0].label, "release-notes");
    }

    #[test]
    fn delete_set_json_carries_the_rows_not_just_counts() {
        let conn = Connection::open_in_memory().unwrap();
        seed_db(&conn);

        let json = collect_purge_delete_set(&conn, "test-project")
            .unwrap()
            .to_json();

        assert_eq!(json["total"], 5);
        assert_eq!(json["tasks"][0]["id"], "cas-0001");
        assert_eq!(json["tasks"][0]["label"], "Fix the purge guard");
        assert_eq!(json["entries"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn delete_set_tolerates_a_database_missing_a_table() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE tasks (id TEXT PRIMARY KEY, title TEXT NOT NULL);")
            .unwrap();
        conn.execute("INSERT INTO tasks VALUES ('cas-1', 'only table')", [])
            .unwrap();

        let set = collect_purge_delete_set(&conn, "test-project").unwrap();

        assert!(
            set.tasks.is_empty(),
            "tasks without attribution are retained"
        );
        assert!(set.entries.is_empty());
        assert_eq!(set.dependencies, 0);
    }

    // ── AC2: the guard refuses destructive runs and names the reason ───────────

    #[test]
    fn fresh_pull_with_empty_queue_is_safe() {
        let refusals = evaluate_purge_safety(Some("2026-08-06T12:00:00Z"), &[], now(), 7);
        assert!(refusals.is_empty(), "unexpected refusals: {refusals:?}");
    }

    #[test]
    fn months_old_pull_is_refused_and_names_the_staleness() {
        let refusals = evaluate_purge_safety(Some("2026-05-01T00:00:00Z"), &[], now(), 7);

        assert_eq!(refusals.len(), 1);
        assert!(matches!(
            refusals[0],
            PurgeRefusal::StalePull {
                age_days: 98,
                threshold_days: 7,
                ..
            }
        ));
        assert_eq!(refusals[0].code(), "stale_pull");
        let reason = refusals[0].reason();
        assert!(reason.contains("stale cloud sync"), "{reason}");
        assert!(reason.contains("98 days ago"), "{reason}");
        assert!(reason.contains("2026-05-01T00:00:00Z"), "{reason}");
    }

    #[test]
    fn staleness_threshold_boundary_is_inclusive_of_the_threshold_day() {
        // Exactly at the threshold is still allowed; one day past is not.
        let at = evaluate_purge_safety(Some("2026-07-31T00:00:00Z"), &[], now(), 7);
        assert!(
            at.is_empty(),
            "7 days old should pass a 7-day threshold: {at:?}"
        );

        let past = evaluate_purge_safety(Some("2026-07-30T00:00:00Z"), &[], now(), 7);
        assert!(matches!(past.as_slice(), [PurgeRefusal::StalePull { .. }]));
    }

    #[test]
    fn never_pulled_is_refused() {
        for missing in [None, Some(""), Some("   ")] {
            let refusals = evaluate_purge_safety(missing, &[], now(), 7);
            assert_eq!(refusals.len(), 1, "for {missing:?}");
            assert_eq!(refusals[0].code(), "never_pulled");
            assert!(refusals[0].reason().contains("no successful cloud pull"));
        }
    }

    #[test]
    fn unparseable_pull_timestamp_is_refused_rather_than_assumed_fresh() {
        let refusals = evaluate_purge_safety(Some("last tuesday"), &[], now(), 7);

        assert_eq!(refusals.len(), 1);
        assert_eq!(refusals[0].code(), "unreadable_pull_timestamp");
        assert!(refusals[0].reason().contains("unknown is not safe"));
    }

    #[test]
    fn naive_timestamp_format_is_understood() {
        assert!(parse_sync_timestamp("2026-08-06 12:00:00").is_some());
        assert!(parse_sync_timestamp("2026-08-06T12:00:00.123").is_some());
        assert!(parse_sync_timestamp("2026-08-06T12:00:00Z").is_some());
        assert!(parse_sync_timestamp("not a date").is_none());
    }

    #[test]
    fn unpushed_local_rows_are_refused_and_sampled_in_the_reason() {
        let pending = vec![
            ("entry".to_string(), "e1".to_string()),
            ("task".to_string(), "cas-0001".to_string()),
        ];

        let refusals = evaluate_purge_safety(Some("2026-08-06T12:00:00Z"), &pending, now(), 7);

        assert_eq!(refusals.len(), 1, "pull is fresh, only the queue is dirty");
        assert_eq!(refusals[0].code(), "unpushed_rows");
        let reason = refusals[0].reason();
        assert!(reason.contains("2 local change(s)"), "{reason}");
        assert!(reason.contains("entry:e1"), "{reason}");
        assert!(reason.contains("task:cas-0001"), "{reason}");
    }

    #[test]
    fn stale_and_unpushed_are_reported_together() {
        let pending = vec![("rule".to_string(), "r1".to_string())];
        let refusals = evaluate_purge_safety(Some("2026-01-01T00:00:00Z"), &pending, now(), 7);

        let codes: Vec<_> = refusals.iter().map(|r| r.code()).collect();
        assert_eq!(codes, vec!["stale_pull", "unpushed_rows"]);
    }

    #[test]
    fn pending_pushes_read_only_content_kinds_from_the_queue() {
        let conn = Connection::open_in_memory().unwrap();
        seed_db(&conn);
        conn.execute_batch(
            r#"
            INSERT INTO sync_queue (entity_type, entity_id, operation) VALUES
                ('entry', 'e1', 'create'),
                ('Task', 'cas-0001', 'update'),
                ('event', 'ev1', 'create');
            "#,
        )
        .unwrap();

        let pending = pending_content_pushes(&conn).unwrap();

        // Task/rule/skill content kinds only (case-insensitively); entry access
        // refreshes and events survive a purge and must not trip the guard.
        assert_eq!(pending, vec![("Task".to_string(), "cas-0001".to_string())]);
    }

    #[test]
    fn pending_purge_rows_do_not_block_but_real_local_edit_still_does() {
        let conn = Connection::open_in_memory().unwrap();
        seed_db(&conn);
        conn.execute_batch(
            r#"
            INSERT INTO sync_queue (entity_type, entity_id, operation) VALUES
                ('entry', 'e1', 'update'),
                ('task', 'cas-0001', 'update'),
                ('rule', 'r1', 'update'),
                ('skill', 's1', 'update'),
                ('task', 'local-edit', 'update');
            "#,
        )
        .unwrap();
        let delete_set = PurgeDeleteSet {
            entries: vec![PurgeEntity::with_evidence(
                "entry",
                "e1",
                "Local learning",
                "origin_project",
                "other-project",
            )],
            tasks: vec![PurgeEntity::with_evidence(
                "task",
                "cas-0001",
                "Fix the purge guard",
                "origin_project",
                "other-project",
            )],
            rules: vec![PurgeEntity::with_evidence(
                "rule",
                "r1",
                "always verify",
                "origin_project",
                "other-project",
            )],
            skills: vec![PurgeEntity::with_evidence(
                "skill",
                "s1",
                "release-notes",
                "origin_project",
                "other-project",
            )],
            dependencies: 0,
        };

        let pending = pending_content_pushes_excluding(&conn, &delete_set).unwrap();
        assert_eq!(
            pending,
            vec![("task".to_string(), "local-edit".to_string())],
            "queue rows the purge will delete are not local work that can be lost"
        );
    }

    #[test]
    fn session_start_entry_refreshes_do_not_block_purge() {
        let conn = Connection::open_in_memory().unwrap();
        seed_db(&conn);
        conn.execute(
            "INSERT INTO sync_queue (entity_type, entity_id, operation)
             VALUES ('entry', 'e1', 'update')",
            [],
        )
        .unwrap();

        assert!(pending_content_pushes(&conn).unwrap().is_empty());
    }

    #[test]
    fn pending_pushes_is_empty_when_the_queue_table_is_absent() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(pending_content_pushes(&conn).unwrap().is_empty());
    }

    /// The guard must fail CLOSED. An unreadable sync_queue previously reported
    /// "zero pending pushes", which silently disabled the unpushed-rows refusal
    /// in a destructive path — a reassuring wrong answer at the worst moment.
    #[test]
    fn unreadable_sync_queue_surfaces_an_error_instead_of_a_silent_zero() {
        // Schema drift: the table exists but not the columns the guard reads.
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch("CREATE TABLE sync_queue (id INTEGER PRIMARY KEY, junk TEXT);")
            .unwrap();

        let err = pending_content_pushes(&conn).unwrap_err();
        assert!(
            err.to_string().contains("cannot read the sync queue"),
            "{err}"
        );
    }

    #[test]
    fn undecodable_sync_queue_row_surfaces_an_error_instead_of_being_skipped() {
        // A row whose entity_type cannot decode as text must not simply vanish
        // from the pending list — that would shorten it toward a false "safe".
        let conn = Connection::open_in_memory().unwrap();
        seed_db(&conn);
        conn.execute(
            "INSERT INTO sync_queue (entity_type, entity_id, operation)
             VALUES (X'FF', 'cas-0001', 'create')",
            [],
        )
        .unwrap();

        let err = pending_content_pushes(&conn).unwrap_err();
        assert!(err.to_string().contains("unreadable row"), "{err}");
    }

    #[test]
    fn a_readable_but_empty_sync_queue_is_still_a_pass() {
        // Fail-closed must not become fail-always: a healthy empty queue is
        // exactly the state a safe purge runs in.
        let conn = Connection::open_in_memory().unwrap();
        seed_db(&conn);

        assert!(pending_content_pushes(&conn).unwrap().is_empty());
        assert!(
            evaluate_purge_safety(Some("2026-08-06T12:00:00Z"), &[], now(), 7).is_empty(),
            "healthy state must still pass"
        );
    }

    // ── AC3: the backup is crash-safe, not an fs::copy of a live WAL DB ────────

    /// The regression that matters: with WAL enabled and un-checkpointed
    /// commits, `fs::copy` of `cas.db` loses committed rows. `VACUUM INTO`
    /// keeps them.
    #[test]
    fn crash_safe_backup_captures_wal_resident_rows_that_fs_copy_loses() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("cas.db");

        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;")
            .unwrap();
        seed_db(&conn);
        // Committed, but still living in -wal (no checkpoint).
        conn.execute(
            "INSERT INTO tasks (id, title) VALUES ('cas-wal', 'committed into the WAL')",
            [],
        )
        .unwrap();
        assert!(
            db_path.with_extension("db-wal").exists(),
            "test precondition: a -wal sidecar must exist"
        );

        let good = dir.path().join("backup.vacuum.db");
        backup_database_crash_safe(&db_path, &good).unwrap();

        let naive = dir.path().join("backup.fscopy.db");
        std::fs::copy(&db_path, &naive).unwrap();

        // Tolerant read: a copy that lost the WAL may not even have the table,
        // since an un-checkpointed database keeps its schema there too.
        let wal_rows = |p: &Path| -> i64 {
            let c = Connection::open(p).unwrap();
            c.query_row("SELECT COUNT(*) FROM tasks WHERE id = 'cas-wal'", [], |r| {
                r.get(0)
            })
            .unwrap_or(0)
        };

        assert_eq!(
            wal_rows(&good),
            1,
            "VACUUM INTO must include the WAL-resident committed row"
        );
        assert_eq!(
            wal_rows(&naive),
            0,
            "fs::copy of a live WAL DB drops WAL-resident commits — the bug this replaces"
        );
    }

    #[test]
    fn crash_safe_backup_is_a_complete_readable_snapshot() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("cas.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch("PRAGMA journal_mode=WAL;").unwrap();
        seed_db(&conn);

        let backup = dir.path().join("backup.db");
        backup_database_crash_safe(&db_path, &backup).unwrap();

        let restored = Connection::open(&backup).unwrap();
        let set = collect_purge_delete_set(&restored, "test-project").unwrap();
        assert_eq!(
            set.total(),
            5,
            "every purged row is recoverable from backup"
        );
        assert_eq!(
            restored
                .query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "ok"
        );
    }

    #[test]
    fn crash_safe_backup_refuses_to_clobber_an_existing_file() {
        let dir = TempDir::new().unwrap();
        let db_path = dir.path().join("cas.db");
        let conn = Connection::open(&db_path).unwrap();
        seed_db(&conn);

        let backup = dir.path().join("backup.db");
        std::fs::write(&backup, b"earlier backup").unwrap();

        let err = backup_database_crash_safe(&db_path, &backup).unwrap_err();
        assert!(err.to_string().contains("refusing to overwrite"), "{err}");
        assert_eq!(std::fs::read(&backup).unwrap(), b"earlier backup");
    }
}
