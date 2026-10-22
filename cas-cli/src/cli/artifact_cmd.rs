//! `cas artifact publish|show|list` — the one call a worker makes to turn a
//! local file into a durable, citable artifact (cassy#910).
//!
//! Human output follows `cas-cli-craft`: a verdict line first, then the facts
//! that justify it, then one `->` hint. `--json` is a single document and is
//! printed instead of the human form, never alongside it.

use anyhow::Context;
use clap::{Args, Subcommand};
use std::path::{Path, PathBuf};

use crate::artifacts::{
    PublishContext, PublishDisposition, PublishOutcome, cloud::ArtifactUploadClient,
    paths::PublishRoots, publish,
};
use crate::config::Config;
use cas_store::{PublishedArtifact, SqliteArtifactStore};

#[derive(Debug, Clone, Subcommand)]
pub enum ArtifactCommands {
    /// Publish a local file as a durable artifact for a task
    Publish(PublishArgs),
    /// Show one artifact record
    Show(ShowArgs),
    /// Fetch a task's proof log from Cloud and verify its SHA-256
    Fetch(FetchArgs),
    /// List the artifacts published for a task
    List(ListArgs),
    /// Plan (and with --yes, apply) reclaiming closed tasks' artifact
    /// directories and, with --simulators, their iOS simulators; no factory
    /// session needed
    Prune(PruneArgs),
}

#[derive(Debug, Clone, Args)]
pub struct PruneArgs {
    /// Keep artifacts of tasks closed more recently than this (e.g. 14d or 14;
    /// at least 7 days)
    #[arg(long, value_name = "DAYS", default_value = "7d", value_parser = parse_days)]
    pub older_than: i64,
    /// Remove what the plan lists. Without it, prune only prints the plan.
    #[arg(long)]
    pub yes: bool,
    /// Print the plan and remove nothing (the default; accepted for clarity)
    #[arg(long, conflicts_with = "yes")]
    pub dry_run: bool,
    /// Also reclaim this project's task-named iOS simulators (macOS)
    #[arg(long)]
    pub simulators: bool,
}

/// `14d` or `14`, at least the shared retention floor.
fn parse_days(value: &str) -> Result<i64, String> {
    let digits = value.trim().strip_suffix('d').unwrap_or(value.trim());
    let days: i64 = digits
        .parse()
        .map_err(|_| format!("expected a number of days such as 14d, got {value:?}"))?;
    if days < crate::artifacts::retention::MIN_RETENTION_DAYS {
        return Err(format!(
            "retention must be at least {}d after close",
            crate::artifacts::retention::MIN_RETENTION_DAYS
        ));
    }
    Ok(days)
}

#[derive(Debug, Clone, Args)]
pub struct PublishArgs {
    /// Task the artifact belongs to
    #[arg(long)]
    pub task: String,
    /// File to publish. Must be under this task's artifacts directory or the checkout.
    pub path: PathBuf,
}

#[derive(Debug, Clone, Args)]
pub struct ShowArgs {
    /// Artifact record id, e.g. art-7f3a9c21
    pub id: String,
}

#[derive(Debug, Clone, Args)]
pub struct FetchArgs {
    /// Task carrying the published artifact reference
    #[arg(long)]
    pub task: String,
    /// Published artifact id from the proof note
    pub id: String,
}

#[derive(Debug, Clone, Args)]
pub struct ListArgs {
    /// Task whose artifacts to list
    #[arg(long)]
    pub task: String,
}

pub fn execute(
    command: &ArtifactCommands,
    cli: &crate::cli::Cli,
    cas_root: &Path,
) -> anyhow::Result<()> {
    match command {
        ArtifactCommands::Publish(args) => execute_publish(args, cas_root, cli.json),
        ArtifactCommands::Show(args) => execute_show(args, cas_root, cli.json),
        ArtifactCommands::Fetch(args) => {
            let (artifact, path) =
                crate::artifacts::proof::fetch_task(cas_root, &args.task, &args.id)?;
            if cli.json {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "artifact_id": artifact.id, "path": path, "sha256": artifact.sha256,
                        "size_bytes": artifact.size_bytes,
                    }))?
                );
            } else {
                println!("[OK] verified {} - {}", artifact.name, artifact.id);
                println!("{}", path.display());
            }
            Ok(())
        }
        ArtifactCommands::List(args) => execute_list(args, cas_root, cli.json),
        ArtifactCommands::Prune(args) => execute_prune(
            args,
            cas_root,
            cli.json,
            &crate::task_simulators::Simctl::default(),
            chrono::Utc::now(),
        ),
    }
}

/// Build the upload client, or `None` when this installation has no
/// credentials. Absence is a supported state, not an error.
fn upload_client(cas_root: &Path) -> Option<ArtifactUploadClient> {
    crate::artifacts::cloud_client(cas_root)
}

fn execute_publish(args: &PublishArgs, cas_root: &Path, json: bool) -> anyhow::Result<()> {
    let store = SqliteArtifactStore::open(cas_root).context("opening the artifact store")?;
    let config = Config::load(cas_root).unwrap_or_default();
    let artifacts_root =
        crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let context = PublishContext {
        store: &store,
        roots: PublishRoots::new(cas_root, &artifacts_root, &args.task),
        task_id: args.task.clone(),
        cloud: upload_client(cas_root),
    };

    let outcome = publish(&context, &args.path).map_err(|error| anyhow::anyhow!("{error}"))?;

    if json {
        println!("{}", serde_json::to_string_pretty(&publish_json(&outcome))?);
        return Ok(());
    }
    print_publish(&outcome);
    Ok(())
}

fn publish_json(outcome: &PublishOutcome) -> serde_json::Value {
    let artifact = &outcome.artifact;
    let (storage, detail) = match &outcome.disposition {
        PublishDisposition::Committed { .. } => ("committed", None),
        PublishDisposition::StorageNotLive { reason } => ("not_live", Some(reason.clone())),
        PublishDisposition::NotLoggedIn => ("not_logged_in", None),
        PublishDisposition::UploadFailed { reason } => ("upload_failed", Some(reason.clone())),
    };
    serde_json::json!({
        "artifact_id": artifact.id,
        "task_id": artifact.task_id,
        "name": artifact.name,
        "mime": artifact.mime,
        "size_bytes": artifact.size_bytes,
        "sha256": artifact.sha256,
        "status": artifact.status,
        "storage": storage,
        "storage_detail": detail,
        "cloud_url": artifact.cloud_url,
        "source": outcome.source.display().to_string(),
    })
}

fn print_publish(outcome: &PublishOutcome) {
    let artifact = &outcome.artifact;
    let verdict = if outcome.disposition.is_committed() {
        "[OK] published"
    } else {
        "[WARN] local-only"
    };
    println!(
        "{verdict} {} - {} - {}",
        artifact.name,
        human_size(artifact.size_bytes),
        artifact.id
    );
    println!("     {}", outcome.disposition.summary());
    println!("     sha256 {}", artifact.sha256);
    match &outcome.disposition {
        PublishDisposition::UploadFailed { reason } => {
            // The failure detail is the actionable part; it is printed in full,
            // indented, rather than truncated into the summary line.
            for line in crate::cloud::presence::wrap_line(reason).lines() {
                println!("     {line}");
            }
            println!("  -> cas artifact show {}", artifact.id);
        }
        PublishDisposition::StorageNotLive { .. } | PublishDisposition::NotLoggedIn => {
            println!("  -> cas artifact show {}", artifact.id);
        }
        PublishDisposition::Committed { .. } => {}
    }
}

fn execute_show(args: &ShowArgs, cas_root: &Path, json: bool) -> anyhow::Result<()> {
    let store = SqliteArtifactStore::open(cas_root).context("opening the artifact store")?;
    let Some(artifact) = store.get(&args.id)? else {
        anyhow::bail!("no artifact {}", args.id);
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&artifact)?);
        return Ok(());
    }
    println!(
        "[OK] {} - {} - {}",
        artifact.name,
        human_size(artifact.size_bytes),
        artifact.id
    );
    println!("     task    {}", artifact.task_id);
    println!("     type    {}", artifact.mime);
    println!("     sha256  {}", artifact.sha256);
    println!("     status  {}", artifact.status);
    if let Some(cloud_id) = &artifact.cloud_artifact_id {
        println!("     cloud   {cloud_id}");
    }
    if let Some(url) = &artifact.cloud_url {
        println!("     url     {url}");
    }
    if let Some(permalink) = &artifact.slack_permalink {
        println!("     slack   {permalink}");
    }
    println!("     created {}", artifact.created_at);
    Ok(())
}

fn execute_list(args: &ListArgs, cas_root: &Path, json: bool) -> anyhow::Result<()> {
    let store = SqliteArtifactStore::open(cas_root).context("opening the artifact store")?;
    let artifacts = store.list_for_task(&args.task)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&artifacts)?);
        return Ok(());
    }
    if artifacts.is_empty() {
        println!("[OK] no artifacts published for {}", args.task);
        println!("  -> cas artifact publish --task {} <path>", args.task);
        return Ok(());
    }
    println!("[OK] {} artifact(s) for {}", artifacts.len(), args.task);
    let width = artifacts
        .iter()
        .map(|artifact| artifact.name.chars().count())
        .max()
        .unwrap_or(0)
        .min(40);
    for artifact in &artifacts {
        println!(
            "     {:<width$}  {:>9}  {:<9}  {}",
            truncate(&artifact.name, width),
            human_size(artifact.size_bytes),
            artifact.status,
            artifact.id,
            width = width
        );
    }
    println!("  -> cas artifact show {}", artifacts[0].id);
    Ok(())
}

/// cas-75a8 (GH #1295): the session-free form of `gc_cleanup artifacts=true`,
/// under the same retention rule. Only this project's closed tasks are
/// candidates; open tasks, other projects' ids and strays are never touched.
fn execute_prune(
    args: &PruneArgs,
    cas_root: &Path,
    json: bool,
    simctl: &crate::task_simulators::Simctl,
    now: chrono::DateTime<chrono::Utc>,
) -> anyhow::Result<()> {
    use crate::artifacts::retention::{Kept, apply_prune, plan_prune};
    let config = Config::load(cas_root).unwrap_or_default();
    let base = crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref());
    let root = crate::config::project_factory_artifacts_root(cas_root, &base);
    // cas-6162: legacy `<base>/cas-<id>/` dirs and task-named simulators
    // carry no project; prove the id is not another known project's.
    let others = crate::task_ownership::KnownProjects::load(cas_root);
    let tasks = crate::store::open_task_store(cas_root)
        .context("opening the task store")?
        .list(None)
        .context("listing tasks")?;
    let plan = plan_prune(cas_root, &root, Some(&base), &others, &tasks, now, args.older_than)
        .map_err(|error| anyhow::anyhow!("artifact retention evidence unavailable; nothing removed: {error}"))?;
    let simulators = args
        .simulators
        .then(|| crate::task_simulators::inventory(simctl, &tasks, &others, now));
    // Plan-only unless --yes (cas-75a8 review): deleting evidence is never
    // the default, and --dry-run is an accepted alias for that default.
    let apply = args.yes && !args.dry_run;
    let (removed, freed, errors) = if apply { apply_prune(&plan) } else { (0, 0, Vec::new()) };
    let (sims_deleted, sims_failed) = match (&simulators, apply) {
        (Some(Ok(list)), true) => crate::task_simulators::reclaim(simctl, list),
        _ => (Vec::new(), Vec::new()),
    };
    let age = |closed: Option<chrono::DateTime<chrono::Utc>>| {
        closed.map_or_else(|| "close date unknown".to_string(), |at| format!("closed {}d ago", (now - at).num_days()))
    };

    if json {
        let dirs = |list: Vec<(&crate::artifacts::retention::TaskArtifactDir, Option<String>)>| {
            list.into_iter()
                .map(|(dir, kept)| {
                    serde_json::json!({
                        "task_id": dir.task_id,
                        "path": dir.path,
                        "bytes": dir.bytes,
                        "closed_at": dir.closed_at,
                        "legacy": dir.legacy,
                        "kept": kept,
                    })
                })
                .collect::<Vec<_>>()
        };
        let document = serde_json::json!({
            "root": plan.root,
            "retention_days": plan.retention_days,
            "applied": apply,
            "reclaimable": dirs(plan.reclaimable.iter().map(|dir| (dir, None)).collect()),
            "reclaimable_bytes": plan.reclaimable_bytes(),
            "kept": dirs(plan.kept.iter().map(|(dir, why)| (dir, Some(match why {
                Kept::Recent => "recent".to_string(),
                Kept::Referenced => "referenced".to_string(),
                Kept::Unproven(reason) => format!("unproven: {reason}"),
            }))).collect()),
            "untouched_dirs": plan.untouched,
            "removed": removed,
            "freed_bytes": freed,
            "errors": errors,
            "simulators": simulators.as_ref().map(|result| match result {
                Ok(list) => serde_json::json!(list.iter().map(|sim| serde_json::json!({
                    "name": sim.device.name,
                    "udid": sim.device.udid,
                    "task_id": sim.task_id,
                    "state": sim.device.state,
                    "bytes": sim.device.bytes,
                    "verdict": sim.verdict.label(),
                })).collect::<Vec<_>>()),
                Err(error) => serde_json::json!({"error": format!("{error:#}")}),
            }),
            "simulators_deleted": sims_deleted,
            "simulator_errors": sims_failed,
        });
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
    }

    let verb = if apply { "reclaimed" } else { "would reclaim" };
    let count = if apply { removed } else { plan.reclaimable.len() };
    let bytes = if apply { freed } else { plan.reclaimable_bytes() };
    println!(
        "[OK] {verb} {count} closed-task artifact dir(s), {} (closed >= {}d, uncited)",
        human_size(bytes),
        plan.retention_days
    );
    println!("     root {}", plan.root.display());
    let place = |dir: &crate::artifacts::retention::TaskArtifactDir| if dir.legacy { " (legacy)" } else { "" };
    for dir in &plan.reclaimable {
        println!("     {:<22} {:>9}  {}{}", dir.task_id, human_size(dir.bytes), age(dir.closed_at), place(dir));
    }
    let mut unproven = 0;
    for (dir, why) in &plan.kept {
        let why = match why {
            Kept::Recent => "kept: closed recently".to_string(),
            Kept::Referenced => "kept: still cited".to_string(),
            Kept::Unproven(reason) => {
                unproven += 1;
                if unproven > 1 && reason.starts_with("task stores of other known projects") {
                    // One shared reason; print it once.
                    "kept: ownership unproven (see above)".to_string()
                } else {
                    format!("kept: ownership unproven: {reason}")
                }
            }
        };
        println!("     {:<22} {:>9}  {why}{}", dir.task_id, human_size(dir.bytes), place(dir));
    }
    if plan.untouched > 0 {
        println!("     {} other dir(s) untouched (open tasks, other projects, strays)", plan.untouched);
    }
    for error in &errors {
        println!("[WARN] could not remove {error}");
    }
    if let Some(simulators) = &simulators {
        print!("{}", crate::task_simulators::render(simulators));
        if apply {
            println!("     simulators deleted: {}", sims_deleted.len());
        }
        for failure in &sims_failed {
            println!("[WARN] simulator delete failed: {failure}");
        }
    }
    if !apply && !plan.reclaimable.is_empty() {
        println!("  -> rerun with --yes to remove them");
    }
    Ok(())
}

/// Decimal units, matching what a file dialog shows the operator.
fn human_size(bytes: u64) -> String {
    const KB: u64 = 1_000;
    const MB: u64 = 1_000_000;
    const GB: u64 = 1_000_000_000;
    match bytes {
        bytes if bytes < KB => format!("{bytes} B"),
        bytes if bytes < MB => format!("{} KB", bytes.div_ceil(KB)),
        bytes if bytes < GB => format!("{:.1} MB", bytes as f64 / MB as f64),
        bytes => format!("{:.1} GB", bytes as f64 / GB as f64),
    }
}

fn truncate(value: &str, width: usize) -> String {
    if value.chars().count() <= width {
        return value.to_string();
    }
    let head: String = value.chars().take(width.saturating_sub(1)).collect();
    format!("{head}…")
}

/// Rendered by both the CLI and the MCP tool so the two surfaces cannot drift.
pub fn render_artifact_line(artifact: &PublishedArtifact) -> String {
    format!(
        "{} - {} - {} [{}]",
        artifact.name,
        human_size(artifact.size_bytes),
        artifact.id,
        artifact.status
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cas-6162: legacy `<artifacts_root>/cas-<id>/` dirs are reachable. One
    /// whose id is closed here and held by no other known project is
    /// reclaimed with --yes; one whose id another project also holds is kept
    /// as unproven, plan-only by default.
    #[test]
    fn legacy_top_level_dirs_are_reclaimed_only_when_provably_this_projects_cas_6162() {
        let env = crate::test_env_guard::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let other_root = crate::store::init_cas_dir(other.path()).unwrap();
        let now = chrono::Utc::now();
        let closed = |id: &str| {
            let mut task = cas_types::Task::new(id.into(), id.into());
            task.status = cas_types::TaskStatus::Closed;
            task.closed_at = Some(now - chrono::Duration::days(30));
            task.updated_at = now - chrono::Duration::days(30);
            task
        };
        let store = crate::store::open_task_store_local(&cas_root).unwrap();
        store.add(&closed("cas-a0a0")).unwrap();
        store.add(&closed("cas-b1b1")).unwrap();
        crate::store::open_task_store_local(&other_root).unwrap().add(&closed("cas-b1b1")).unwrap();
        for (root, id) in [(&cas_root, "fixture-this"), (&other_root, "fixture-other")] {
            std::fs::write(root.join("config.toml"), format!("[project]\ncanonical_id = {id:?}\n")).unwrap();
        }
        crate::store::known_repos::ensure_host_schema().unwrap();
        crate::store::known_repos::register_repo_strict(project.path()).unwrap();
        crate::store::known_repos::register_repo_strict(other.path()).unwrap();
        let base = crate::config::resolved_factory_artifacts_root(None);
        assert!(base.starts_with(env.home()), "{}", base.display());
        for id in ["cas-a0a0", "cas-b1b1", "cas-c2c2"] {
            std::fs::create_dir_all(base.join(id)).unwrap();
            std::fs::write(base.join(id).join("proof.zip"), b"zip").unwrap();
        }
        let missing = crate::task_simulators::Simctl::with_binary(env.home().join("no-xcrun"));
        let plan_only = PruneArgs { older_than: 7, yes: false, dry_run: false, simulators: false };
        execute_prune(&plan_only, &cas_root, false, &missing, now).unwrap();
        assert!(base.join("cas-a0a0").exists(), "plan-only by default");

        let tasks = store.list(None).unwrap();
        let others = crate::task_ownership::KnownProjects::load(&cas_root);
        let root = crate::config::project_factory_artifacts_root(&cas_root, &base);
        let plan = crate::artifacts::retention::plan_prune(&cas_root, &root, Some(&base), &others, &tasks, now, 7).unwrap();
        assert_eq!(
            plan.reclaimable.iter().map(|dir| (dir.task_id.as_str(), dir.legacy)).collect::<Vec<_>>(),
            vec![("cas-a0a0", true)],
            "kept: {:?}",
            plan.kept
        );
        assert!(matches!(&plan.kept[..], [(dir, crate::artifacts::retention::Kept::Unproven(why))]
            if dir.task_id == "cas-b1b1" && why.contains(&other.path().display().to_string())), "{:?}", plan.kept);
        assert_eq!(plan.untouched, 1, "cas-c2c2 is no task of this project");

        let yes = PruneArgs { older_than: 7, yes: true, dry_run: false, simulators: false };
        execute_prune(&yes, &cas_root, false, &missing, now).unwrap();
        assert!(!base.join("cas-a0a0").exists());
        assert!(base.join("cas-b1b1").exists() && base.join("cas-c2c2").exists());
    }

    /// cas-75a8 (GH #1295): prune only plans unless --yes (with or without
    /// --dry-run), and --yes reclaims only this project's closed, uncited
    /// tasks past retention.
    #[test]
    fn prune_reclaims_only_old_uncited_closed_task_dirs_cas_75a8() {
        let env = crate::test_env_guard::TestEnvGuard::temp_home();
        let project = tempfile::tempdir().unwrap();
        let cas_root = crate::store::init_cas_dir(project.path()).unwrap();
        let now = chrono::Utc::now();
        let store = crate::store::open_task_store_local(&cas_root).unwrap();
        let add = |id: &str, closed_days: Option<i64>, notes: &str| {
            let mut task = cas_types::Task::new(id.into(), id.into());
            task.notes = notes.into();
            if let Some(days) = closed_days {
                task.status = cas_types::TaskStatus::Closed;
                task.closed_at = Some(now - chrono::Duration::days(days));
                task.updated_at = now - chrono::Duration::days(days);
            }
            store.add(&task).unwrap();
        };
        add("cas-0old", Some(30), "");
        add("cas-0new", Some(2), "");
        add("cas-0cit", Some(30), "");
        add("cas-0opn", None, "evidence lives in cas-0cit");
        let config = Config::load(&cas_root).unwrap_or_default();
        let root = crate::config::project_factory_artifacts_root(
            &cas_root,
            &crate::config::resolved_factory_artifacts_root(config.factory().artifacts_root.as_deref()),
        );
        assert!(root.starts_with(env.home()), "{}", root.display());
        for id in ["cas-0old", "cas-0new", "cas-0cit", "cas-0opn", "cas-0zzz"] {
            std::fs::create_dir_all(root.join(id)).unwrap();
            std::fs::write(root.join(id).join("evidence.log"), b"proof").unwrap();
        }
        let missing = crate::task_simulators::Simctl::with_binary(env.home().join("no-xcrun"));
        for plan_only in [
            PruneArgs { older_than: 7, yes: false, dry_run: false, simulators: false },
            PruneArgs { older_than: 7, yes: false, dry_run: true, simulators: false },
        ] {
            execute_prune(&plan_only, &cas_root, true, &missing, now).unwrap();
            assert!(root.join("cas-0old").exists(), "without --yes prune only plans");
        }

        let tasks = store.list(None).unwrap();
        let none = crate::task_ownership::KnownProjects::from_roots(&cas_root, Vec::new());
        let plan = crate::artifacts::retention::plan_prune(&cas_root, &root, None, &none, &tasks, now, 7).unwrap();
        let ids = |dirs: Vec<&str>| dirs.into_iter().map(str::to_string).collect::<Vec<_>>();
        assert_eq!(plan.reclaimable.iter().map(|dir| dir.task_id.clone()).collect::<Vec<_>>(), ids(vec!["cas-0old"]));
        assert_eq!(plan.reclaimable_bytes(), 5);
        assert_eq!(plan.kept.len(), 2, "recent and cited closed tasks are kept");
        assert_eq!(plan.untouched, 2, "the open task and the unknown id are untouched");

        let wet = PruneArgs { older_than: 7, yes: true, dry_run: false, simulators: false };
        execute_prune(&wet, &cas_root, true, &missing, now).unwrap();
        assert!(!root.join("cas-0old").exists());
        for kept in ["cas-0new", "cas-0cit", "cas-0opn", "cas-0zzz"] {
            assert!(root.join(kept).exists(), "{kept} must survive");
        }
        assert!(parse_days("6d").is_err() && parse_days("14d") == Ok(14) && parse_days("14") == Ok(14));
    }

    #[test]
    fn sizes_render_in_the_units_a_file_dialog_uses() {
        assert_eq!(human_size(0), "0 B");
        assert_eq!(human_size(999), "999 B");
        assert_eq!(human_size(1_000), "1 KB");
        assert_eq!(human_size(812_345), "813 KB");
        assert_eq!(human_size(3_400_000), "3.4 MB");
        assert_eq!(human_size(42_800_000_000), "42.8 GB");
    }

    #[test]
    fn long_names_are_truncated_with_an_ellipsis_not_cut() {
        assert_eq!(truncate("short.pdf", 20), "short.pdf");
        let long = "a-very-long-artifact-name-indeed.pdf";
        let truncated = truncate(long, 10);
        assert_eq!(truncated.chars().count(), 10);
        assert!(truncated.ends_with('…'));
    }

    #[test]
    fn the_shared_line_names_the_facts_both_surfaces_must_agree_on() {
        let artifact = PublishedArtifact {
            id: "art-1".to_string(),
            task_id: "cas-b72a".to_string(),
            name: "brief.pdf".to_string(),
            mime: "application/pdf".to_string(),
            size_bytes: 812_345,
            sha256: "a".repeat(64),
            cloud_artifact_id: None,
            status: "local".to_string(),
            cloud_url: None,
            slack_permalink: None,
            created_at: "2026-09-18T20:00:00Z".to_string(),
        };
        let line = render_artifact_line(&artifact);
        assert!(line.contains("brief.pdf") && line.contains("art-1") && line.contains("local"));
        assert!(line.contains("813 KB"));
    }
}
