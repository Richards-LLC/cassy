//! Release script adapters. Train selection and notice delivery stay in core.
use crate::qa_batch::{ReleaseInventory, ReleaseProofRef, ReleaseScope};
use std::collections::BTreeMap;
use crate::store::{open_agent_store, open_reminder_store, open_task_store};
use anyhow::{Context, Result, ensure};
use clap::Args;
use cas_types::{TaskStatus, TaskType};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

#[derive(Debug, Args)]
pub struct ContextArgs {
    #[arg(long, conflicts_with = "version")]
    branch: Option<String>,
    #[arg(long)]
    version: Option<String>,
}
#[derive(Debug, Args)]
pub struct InventoryArgs {
    /// Portable ReleaseScope JSON identifying an exact released commit
    #[arg(long)]
    scope: PathBuf,
}

/// The CLI and trusted release adapters share the same read-only resolver.
pub fn inventory(args: &InventoryArgs, root: &Path) -> Result<()> {
    let scope: ReleaseScope = serde_json::from_slice(&fs::read(&args.scope)?)?;
    let repo = root.parent().context("project checkout unavailable")?;
    let inventory = crate::qa_batch::resolve_release_inventory(root, repo, &scope)
        .map_err(anyhow::Error::msg)?;
    println!("{}", serde_json::to_string(&inventory)?);
    Ok(())
}

#[derive(Debug, Args)]
pub struct TrackArgs {
    #[arg(long)]
    commit: String,
    #[arg(long)]
    version: String,
    #[arg(long)]
    first_child: String,
    /// Audited constituent refs, deferrals and included delivery IDs
    #[arg(long)]
    inputs: Option<PathBuf>,
}
#[derive(Debug, Args)]
pub struct InstalledArgs {
    #[arg(long)]
    receipt: PathBuf,
    #[arg(long)]
    repo: String,
    /// Report exhausted installer attempts once, without retrying reconciliation
    #[arg(long)]
    failed_attempts: Option<std::num::NonZeroU32>,
}
#[derive(Debug, Args)]
pub struct BackfillArgs {
    /// Show the reconciliation and release-tag close verdicts without writing.
    #[arg(long)]
    dry_run: bool,
}
/// Audited declaration carried unchanged from cut inputs into publication.
/// Unknown audit metadata is retained for older receipts and producer extensions.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(super) struct ReleaseInputs {
    #[serde(default)]
    pub(super) checked_sha: String,
    #[serde(default)]
    pub(super) included_task_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) no_code_members: BTreeMap<String, Vec<ReleaseProofRef>>,
    #[serde(flatten)]
    metadata: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(super) struct ReleaseContext {
    pub(super) train_id: String,
    pub(super) owner: String,
    pub(super) repository: String,
    branch: String,
    integration: String,
    release_commit: Option<String>,
    first_child_commit: Option<String>,
    first_child_epoch: Option<i64>,
    landing_reminder: Option<i64>,
    publish_reminder: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) release_inputs: Option<ReleaseInputs>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) release_scope: Option<ReleaseScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    release_inventory: Option<ReleaseInventory>,
}

fn version_ok(version: &str) -> bool {
    let fields: Vec<_> = version.split('.').collect();
    fields.len() == 3
        && fields
            .iter()
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}
pub(super) fn receipt_path(root: &Path, version: &str) -> Result<PathBuf> {
    ensure!(version_ok(version), "expected version x.y.z");
    Ok(root.join("releases").join(format!("v{version}.json")))
}
fn save(path: &Path, context: &ReleaseContext) -> Result<()> {
    let dir = path.parent().context("release receipt directory")?;
    fs::create_dir_all(dir)?;
    let mut tmp = tempfile::NamedTempFile::new_in(dir)?;
    use std::io::Write;
    tmp.write_all(&serde_json::to_vec_pretty(context)?)?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)?;
    Ok(())
}
pub(super) fn git(repo: &Path, args: &[&str]) -> Result<String> {
    git_with_timeout(repo, args, std::time::Duration::from_secs(30))
}

fn git_with_timeout(repo: &Path, args: &[&str], timeout: std::time::Duration) -> Result<String> {
    let mut command = Command::new("git");
    command.arg("-C").arg(repo).args(args).env("GIT_TERMINAL_PROMPT", "0");
    // A credential helper or remote can hang while the parent polls its pipes.
    // Capture to files and kill the owned Git process group on the deadline.
    let out = crate::bounded_process::run_command(
        &mut command,
        crate::bounded_process::Deadline::after(timeout),
        timeout,
    ).map_err(|error| anyhow::anyhow!("git {}: {error:?}", args.join(" ")))?;
    ensure!(
        out.status.success(),
        "git {}: {}",
        args.join(" "),
        String::from_utf8_lossy(&out.stderr)
    );
    Ok(String::from_utf8(out.stdout)?.trim().to_string())
}

/// Single seam to the canonical oldest open train. Never closes or selects by GitHub labels.
fn open_train(root: &Path) -> Result<(PathBuf, cas_types::Task)> {
    use crate::mcp::tools::core::task::{release_train, repo_context};
    let (repo, integration) =
        repo_context::integration_branch(root).context("no integration branch")?;
    let store = open_task_store(root)?;
    // The project store can live in the main checkout while the cut runs in
    // a train worktree. Validate repository identity, then use the caller's tree.
    let caller = PathBuf::from(git(
        &std::env::current_dir()?,
        &["rev-parse", "--show-toplevel"],
    )?);
    ensure!(
        git(
            &repo,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"]
        )? == git(
            &caller,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"]
        )?,
        "release checkout belongs to another repository"
    );
    let branch = git(&caller, &["branch", "--show-current"])?;
    let train = release_train::active_trains(store.as_ref(), &integration)
        .into_iter()
        .find(|train| train.effective_epic_branch() == Some(branch.as_str()))
        .or_else(|| release_train::open_trains(store.as_ref(), &integration).into_iter().next())
        .context("no open release train; create work on the train first")?;
    Ok((caller, train))
}
fn new_context(repo: &Path, train: &cas_types::Task) -> Result<ReleaseContext> {
    let branch = train
        .effective_epic_branch()
        .context("train has no lane")?
        .to_string();
    let integration = train
        .deliverables
        .work_target
        .as_ref()
        .context("train has no integration target")?
        .target_branch
        .clone();
    let owner = train
        .epic_verification_owner
        .clone()
        .unwrap_or_else(|| "operator".into());
    let repository = crate::github_repo::origin_slug(repo, std::time::Duration::from_secs(3))
        .map_err(anyhow::Error::msg)?
        .context("release checkout has no GitHub origin")?;
    Ok(ReleaseContext {
        train_id: train.id.clone(),
        owner,
        repository,
        branch,
        integration,
        release_commit: None,
        first_child_commit: None,
        first_child_epoch: None,
        landing_reminder: None,
        publish_reminder: None,
        release_inputs: None,
        release_scope: None,
        release_inventory: None,
    })
}

pub fn context(args: &ContextArgs, root: &Path) -> Result<()> {
    let value: ReleaseContext = if let Some(version) = &args.version {
        serde_json::from_slice(&fs::read(receipt_path(root, version)?)?)?
    } else {
        let (repo, train) = open_train(root)?;
        let value = new_context(&repo, &train)?;
        if let Some(branch) = &args.branch {
            ensure!(
                branch == &value.branch,
                "check out the open release train {} first",
                value.branch
            );
        }
        value
    };
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}

pub fn track(args: &TrackArgs, root: &Path) -> Result<()> {
    let (repo, train) = open_train(root)?;
    track_on_train(args, root, &repo, &train)
}
fn track_on_train(
    args: &TrackArgs,
    root: &Path,
    repo: &Path,
    train: &cas_types::Task,
) -> Result<()> {
    let mut expected = new_context(repo, train)?;
    let operations = crate::worktree::GitOperations::new(repo.to_path_buf());
    let canonical_repo = operations.canonical_repo_key()?;
    let _target_lock = crate::worktree::target_lock::lock_delivery_target(root, &canonical_repo, &expected.branch)?;
    ensure!(
        git(&repo, &["branch", "--show-current"])? == expected.branch,
        "release must be cut on the train branch"
    );
    ensure!(
        args.commit.len() == 40 && args.commit.bytes().all(|c| c.is_ascii_hexdigit()),
        "release commit must be a full SHA"
    );
    ensure!(
        git(&repo, &["rev-parse", "HEAD"])? == args.commit,
        "release commit must equal the train tip"
    );
    let path = receipt_path(root, &args.version)?;
    fs::create_dir_all(path.parent().unwrap())?;
    let lock = fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.parent().unwrap().join("tracking.lock"))?;
    fs2::FileExt::lock_exclusive(&lock)?;
    if path.exists() {
        expected = serde_json::from_slice(&fs::read(&path)?)?;
        ensure!(
            expected.train_id == train.id,
            "version already belongs to another train"
        );
    }
    if let Some(path) = &args.inputs {
        let audit: ReleaseInputs = serde_json::from_slice(&fs::read(path)?)?;
        let checked = audit.checked_sha.as_str();
        ensure!(!checked.is_empty(), "release input audit has no checked_sha");
        ensure!(checked == args.commit || git(repo, &["rev-parse", &format!("{}^1", args.commit)])
            .ok().as_deref() == Some(checked), "release input audit is stale; rerun release.sh");
        expected.release_inputs = Some(audit);
    }
    let previous_commit = expected.release_commit.replace(args.commit.clone());
    if expected.first_child_commit.is_none() {
        ensure!(
            args.first_child.len() == 40 && args.first_child.bytes().all(|b| b.is_ascii_hexdigit()),
            "first child must be a full SHA"
        );
        git(
            &repo,
            &[
                "merge-base",
                "--is-ancestor",
                &args.first_child,
                &args.commit,
            ],
        )?;
        expected.first_child_epoch =
            Some(git(&repo, &["show", "-s", "--format=%ct", &args.first_child])?.parse()?);
        expected.first_child_commit = Some(args.first_child.clone());
    }
    // Refuse missing or mismatched explicit report bindings before freezing the
    // lane. Old code-only audits do not need report preflight.
    if let Some(inputs) = &expected.release_inputs {
        let tasks = open_task_store(root)?;
        let reports: std::collections::BTreeSet<_> = inputs.included_task_ids.iter()
            .chain(inputs.no_code_members.keys())
            .filter(|id| tasks.get(id).is_ok_and(|t|
                crate::qa_pass::is_no_code(&t) || t.deliverables.evidence_only.is_some()))
            .cloned().chain(inputs.no_code_members.keys().cloned()).collect();
        if !reports.is_empty() {
            let mut scope = super::release_adapters::tag_scope(
                root, repo, Some(&expected), &train.id, &format!("v{}", args.version), &args.commit,
            )?;
            scope.release_tag = None; // The cut has not published a tag yet.
            for id in reports {
                crate::release_reconcile::validate_no_code_proofs(root, &scope, &id,
                    inputs.no_code_members.get(&id).map(Vec::as_slice).unwrap_or_default())
                    .map_err(anyhow::Error::msg)?;
            }
        }
    }
    // This is the cut boundary, before push/enqueue and independent of
    // reminder availability. New tasks must not join the frozen lane.
    let tasks = open_task_store(root)?;
    let mut cut = tasks.get(&train.id)?;
    if !crate::mcp::tools::core::task::release_train::is_cut_train(&cut) {
        cut.labels.push(crate::mcp::tools::core::task::release_train::RELEASE_TRAIN_CUT_LABEL.into());
        cut.updated_at = chrono::Utc::now();
        tasks.update(&cut).context("could not persist release train cut marker")?;
    }
    let tracking = (|| -> Result<()> {
        let agents = open_agent_store(root)?.list(None)?;
        let recipient =
            crate::supervisor_notices::resolve(&agents, train.epic_verification_owner.as_deref(), None);
        if train.epic_verification_owner.is_none() {
            expected.owner = recipient.id.clone();
        }
        let reminders = open_reminder_store(root)?;
        cancel_superseded_watches(root, &expected.train_id, &args.version, reminders.as_ref())?;
        let mut existing = reminders.list_all_pending()?;
        if previous_commit
            .as_deref()
            .is_some_and(|sha| sha != args.commit)
        {
            if let Some(old) = existing
                .iter()
                .find(|r| Some(r.id) == expected.landing_reminder)
            {
                reminders.cancel(old.id, &old.owner_id)?;
            }
            expected.landing_reminder = None;
        }
        existing.extend(reminders.list_recently_fired(7 * 24 * 3600)?);
        // Save between effects; a crashed retry also recognizes the durable reminder row.
        save(&path, &expected)?;
        if recipient.id == "operator" {
            println!(
                "Release v{} tracking skipped: no reachable train supervisor; cut can continue",
                args.version
            );
            return Ok(());
        }
        let landing = serde_json::json!({"commit": args.commit, "target_branch": expected.integration});
        let publish = serde_json::json!({"tag": format!("v{}", args.version)});
        for (event, filter, slot) in [
            (
                "branch_contained_in",
                landing,
                &mut expected.landing_reminder,
            ),
            ("tag_exists", publish, &mut expected.publish_reminder),
        ] {
            // A repaired train tip has its own landing condition; publication remains one version.
            if slot.is_some() {
                continue;
            }
            let found = existing.iter().find(|r| {
                r.task_id.as_deref() == Some(&train.id)
                    && r.message
                        .starts_with(&format!("Release v{}:", args.version))
                    && r.trigger_event.as_deref() == Some(event)
                    && r.trigger_filter.as_ref() == Some(&filter)
            });
            *slot = Some(if let Some(reminder) = found {
                reminder.id
            } else {
                reminders.create_with_scope(
                    &recipient.id,
                    None,
                    &format!(
                        "Release v{}: {event}; continue the release and inspect its receipt",
                        args.version
                    ),
                    cas_store::ReminderTriggerType::Event,
                    None,
                    Some(event),
                    Some(&filter),
                    0,
                    recipient.session.as_deref(),
                    None,
                    true,
                    Some(&train.id),
                )?
            });
        }
        save(&path, &expected)?;
        println!(
            "Release v{} tracked: cross-session landing #{} and publication #{}",
            args.version,
            expected.landing_reminder.unwrap(),
            expected.publish_reminder.unwrap()
        );
        Ok(())
    })();
    if let Err(error) = tracking {
        eprintln!("Release train {} is frozen; reminder tracking unavailable: {error:#}. The cut can continue.", expected.branch);
    }
    Ok(())
}

/// A new version supersedes both old conditions for this train. The caller
/// holds the project tracking lock, so simultaneous cuts cannot race this scan.
fn cancel_superseded_watches(
    root: &Path,
    train_id: &str,
    version: &str,
    reminders: &dyn cas_store::ReminderStore,
) -> Result<()> {
    let current = semver::Version::parse(version)?;
    let pending = reminders.list_all_pending()?;
    for entry in fs::read_dir(root.join("releases"))? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(previous) = path
            .file_stem()
            .and_then(|s| s.to_str())
            .and_then(|s| s.strip_prefix('v'))
            .and_then(|s| semver::Version::parse(s).ok())
        else {
            continue;
        };
        if previous >= current {
            continue;
        }
        let old: ReleaseContext = serde_json::from_slice(&fs::read(&path)?)?;
        if old.train_id != train_id {
            continue;
        }
        for id in [old.landing_reminder, old.publish_reminder]
            .into_iter()
            .flatten()
        {
            if let Some(row) = pending.iter().find(|r| r.id == id) {
                reminders.cancel(id, &row.owner_id)?;
            }
        }
    }
    Ok(())
}

fn installed_at_root(root: &Path, repo: &str, version: &str) -> Result<bool> {
    let Some(context) = tracked_context(root, repo, version)? else {
        return Ok(false);
    };
    // Load even a closed train: core may already have rolled to the next one.
    let mut train = open_task_store(root)?.get(&context.train_id)?;
    // The cut receipt retains its owner if core already rolled the train over.
    train.epic_verification_owner = Some(context.owner);
    crate::supervisor_notices::release_action(
        root,
        &train,
        crate::supervisor_notices::ReleaseNoticePhase::PostPublish,
        version,
        &format!(
            "v{version} published and installed: post-release steps due (notes checksums, Slack, factory restart, release receipt)"
        ),
    )?;
    let tag = format!("v{version}");
    let reconciled = super::release_adapters::recover(
        root,
        &context.train_id,
        &tag,
        crate::release_reconcile::ReconcileTrigger::Installed,
    )?;
    println!("{reconciled}");
    Ok(true)
}

fn unmerged_factory_commit_count(repo: &Path, child: &cas_types::Task, tag_sha: &str) -> usize {
    let Some(start) = child.deliverables.factory_branch_start.as_ref() else {
        return 0;
    };
    let branch = start.branch.trim_start_matches("refs/heads/");
    let refs = [format!("refs/heads/{branch}"), format!("refs/remotes/origin/{branch}")];
    let Some(branch_ref) = refs.into_iter().find(|reference| {
        git(repo, &["rev-parse", "--verify", "--quiet", "--end-of-options", &format!("{reference}^{{commit}}")]).is_ok()
    }) else {
        return 0;
    };
    git(repo, &["rev-list", "--count", &format!("{tag_sha}..{branch_ref}")])
        .ok()
        .and_then(|count| count.parse::<usize>().ok())
        .unwrap_or(0)
}

fn is_release_train_task(task: &cas_types::Task) -> bool {
    task.task_type == TaskType::Epic
        && (crate::mcp::tools::core::task::release_train::is_release_train(task)
            || task.effective_epic_branch().is_some_and(|lane| lane.starts_with("epic/release-train-")))
}

fn delivery_is_shipped(repo: &Path, child: &cas_types::Task, tag_sha: &str) -> bool {
    let Some(anchor) = crate::qa_batch::delivery_anchor(repo, child) else {
        return false;
    };
    crate::qa_batch::release_tag_ships_delivery(repo, &anchor, tag_sha, &child.id)
}

fn published_later_tags(repo: &Path, tag: &str) -> Result<Vec<(String, String)>> {
    let version = semver::Version::parse(tag.strip_prefix('v').unwrap_or(tag))?;
    let output = git(repo, &["ls-remote", "--tags", "origin"])?;
    let mut published = std::collections::BTreeMap::new();
    for line in output.lines() {
        let Some((sha, reference)) = line.split_once('\t') else { continue };
        let Some(name) = reference.strip_prefix("refs/tags/") else { continue };
        let name = name.strip_suffix("^{}").unwrap_or(name);
        // Git emits the peeled annotated tag after its tag object.
        published.insert(name.to_string(), sha.to_string());
    }
    let mut tags = Vec::new();
    for (name, remote_sha) in published {
        let Ok(candidate) = semver::Version::parse(name.strip_prefix('v').unwrap_or(&name)) else { continue };
        if candidate > version && git(repo, &["rev-parse", "--verify", "--end-of-options", &format!("refs/tags/{name}^{{commit}}")])
            .is_ok_and(|local| local == remote_sha)
        {
            tags.push((candidate, name, remote_sha));
        }
    }
    tags.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
    Ok(tags.into_iter().map(|(_, name, sha)| (name, sha)).collect())
}

fn preview_release_close_hold(root: &Path, child: &cas_types::Task, repo: &Path) -> Option<String> {
    match crate::mcp::tools::core::task::lifecycle::merge_close::release_receipt_close_admission(root, child, repo) {
        Ok(own_review) => own_review,
        Err(reason) => Some(reason),
    }
}

/// Read-only backfill preview. Applying a release is the shared release
/// operation's job (`cas release backfill` without `--dry-run`); this never writes.
fn preview_shipped_train(root: &Path, train_id: &str, tag: &str) -> Result<String> {
    let (repo, _) = crate::mcp::tools::core::task::repo_context::integration_branch(root)
        .context("release train has no integration branch")?;
    let tag_sha = published_tag_sha(&repo, tag)?;
    let store = open_task_store(root)?;
    let train = store.get(train_id)?;
    ensure!(is_release_train_task(&train), "{train_id} is not a release train");
    let lane = train.effective_epic_branch().context("release train has no lane")?;
    let lane_ref = if git(&repo, &["rev-parse", "--verify", &format!("origin/{lane}^{{commit}}")]).is_ok() {
        format!("origin/{lane}")
    } else {
        lane.to_string()
    };
    ensure!(
        git(&repo, &["merge-base", "--is-ancestor", &lane_ref, &tag_sha]).is_ok(),
        "release tag {tag} does not contain train lane {lane}"
    );

    let children = store.get_subtasks(train_id)?;
    let needs_later_tags = children.iter().any(|child| !child.is_terminal()
        && crate::qa_batch::delivery_anchor(&repo, child).is_some()
        && !delivery_is_shipped(&repo, child, &tag_sha));
    let later_tags = if needs_later_tags { published_later_tags(&repo, tag)? } else { Vec::new() };
    let shipped_tags: std::collections::BTreeMap<_, _> = children.iter().filter_map(|child| {
        if delivery_is_shipped(&repo, child, &tag_sha) {
            Some((child.id.clone(), tag.to_string()))
        } else {
            later_tags.iter().find(|(_, sha)| delivery_is_shipped(&repo, child, sha))
                .map(|(later_tag, _)| (child.id.clone(), later_tag.clone()))
        }
    }).collect();
    let mut report = Vec::new();
    let mut held = false;
    for child in &children {
        if child.status == TaskStatus::Cancelled {
            report.push(format!("{}: SKIP (cancelled); gate verdict N/A", child.id));
            continue;
        }
        if let Some(shipped_tag) = shipped_tags.get(&child.id) {
            if child.status == TaskStatus::Closed {
                report.push(format!("{}: SKIP (already closed); gate verdict PASS (tag contains delivery)", child.id));
                continue;
            }
            let receipt = crate::qa_batch::prepare_release_tag_receipt(root, std::slice::from_ref(&child.id), shipped_tag);
            if let Err(error) = receipt {
                held = true;
                report.push(format!("{}: ERROR (release receipt prep: {error}); gate verdict N/A", child.id));
            } else if let Some(reason) = preview_release_close_hold(root, child, &repo) {
                held = true;
                report.push(format!("{}: HOLD ({reason}); ordinary close required", child.id));
            } else {
                report.push(format!("{}: CLOSE pending ordinary close gates; release receipt admission PASS (tag contains delivery)", child.id));
            }
            if child.status != TaskStatus::AwaitingMerge {
                report.push(format!("{}: shipped tree is present; status {} needs its ordinary close", child.id, child.status));
            }
            continue;
        }
        if child.is_terminal() {
            report.push(format!("{}: SKIP (already {}); gate verdict N/A", child.id, child.status));
            continue;
        }
        held = true;
        let unmerged = unmerged_factory_commit_count(&repo, child, &tag_sha);
        report.push(format!(
            "{}: HOLD (not in tag; {unmerged} unmerged commits); named residual of the release operation",
            child.id
        ));
    }
    let mut preview = format!("{train_id} ({tag}) dry-run: no writes");
    for line in &report {
        preview.push_str("\n  ");
        preview.push_str(line);
    }
    let verdict = if held {
        "STAY OPEN (unshipped or held children remain named residuals)"
    } else {
        "CLOSE through the release operation's parent gates"
    };
    preview.push_str(&format!("\n  Train verdict: {verdict}"));
    Ok(preview)
}

pub(super) fn published_tag_sha(repo: &Path, tag: &str) -> Result<String> {
    ensure!(semver::Version::parse(tag.strip_prefix('v').unwrap_or(tag)).is_ok(), "invalid release tag {tag}");
    let local = git(repo, &["rev-parse", "--verify", "--end-of-options", &format!("refs/tags/{tag}^{{commit}}")])?;
    let remote = git(repo, &["ls-remote", "origin", &format!("refs/tags/{tag}"), &format!("refs/tags/{tag}^{{}}")])?;
    let found = remote.lines().any(|line| {
        line.split_whitespace().next() == Some(local.as_str())
    });
    ensure!(found, "tag {tag} is not published on origin at {local}");
    Ok(local)
}

pub fn backfill(args: &BackfillArgs, root: &Path) -> Result<()> {
    let mut failures = Vec::new();
    for (train, tag) in [
        ("cas-c4a0", "v4.3.0"),
        ("cas-baf5", "v4.4.0"),
        ("cas-dcb3", "v4.5.0"),
        ("cas-d7c7", "v4.6.0"),
    ] {
        let result = if args.dry_run {
            preview_shipped_train(root, train, tag)
        } else {
            super::release_adapters::recover(
                root,
                train,
                tag,
                crate::release_reconcile::ReconcileTrigger::Backfill,
            )
        };
        match result {
            Ok(report) => println!("{report}"),
            Err(error) => {
                eprintln!("[ERROR] {train} ({tag}): {error:#}");
                failures.push(train);
            }
        }
    }
    ensure!(failures.is_empty(), "release backfill failed for {}", failures.join(", "));
    Ok(())
}

/// The version of a non-draft GitHub release publication read from `path`.
pub(super) fn published_version(path: &Path) -> Result<String> {
    let publication: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
    let version = publication["tag_name"]
        .as_str()
        .and_then(|s| s.strip_prefix('v'))
        .context("publication has no version tag")?;
    ensure!(version_ok(version), "expected version x.y.z");
    ensure!(
        publication["draft"].as_bool() == Some(false),
        "release is still a draft"
    );
    let published = publication["published_at"]
        .as_str()
        .context("release is not published")?;
    chrono::DateTime::parse_from_rfc3339(published).context("invalid published_at")?;
    Ok(version.to_string())
}

/// This project plus every healthy tracked project on this host.
pub(super) fn release_roots(current_root: Option<&Path>) -> Result<std::collections::BTreeSet<PathBuf>> {
    let mut roots = std::collections::BTreeSet::new();
    if let Some(root) = current_root {
        roots.insert(root.to_path_buf());
    }
    for repo in crate::worktree::discovery::list_tracked_repos()? {
        if repo.healthy {
            roots.insert(crate::store::CasContext::from_path(&repo.path)?.cas_root);
        }
    }
    Ok(roots)
}

/// The tracked cut receipt of `version` for `repo`, if this project cut it.
pub(super) fn tracked_context(root: &Path, repo: &str, version: &str) -> Result<Option<ReleaseContext>> {
    let path = receipt_path(root, version)?;
    if !path.exists() {
        return Ok(None);
    }
    let context: ReleaseContext = serde_json::from_slice(&fs::read(path)?)?;
    Ok(context.repository.eq_ignore_ascii_case(repo).then_some(context))
}

pub fn installed(args: &InstalledArgs, current_root: Option<&Path>) -> Result<()> {
    let version = published_version(&args.receipt)?;
    ensure!(
        semver::Version::parse(&version)? <= semver::Version::parse(env!("CARGO_PKG_VERSION"))?,
        "publication must not be newer than the installed binary"
    );
    let mut delivered = 0;
    for root in release_roots(current_root)? {
        delivered += usize::from(if let Some(attempts) = args.failed_attempts {
            super::release_notice_failure::record(&root, &args.repo, &version, attempts, &args.receipt)?
        } else {
            installed_at_root(&root, &args.repo, &version)?
        });
    }
    if args.failed_attempts.is_some() {
        ensure!(delivered > 0, "no tracked release owner found for {version}; keep the failed receipt for reporting");
        println!("Post-release failure recorded for v{version} ({delivered} project(s))");
    } else {
        println!("Post-release notice recorded for v{version} ({delivered} project(s))");
    }
    Ok(())
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use cas_types::{Agent, AgentRole, Dependency, DependencyType, Task, TaskType, WorkTarget};

    /// Shared with the release adapters' tests under a crate-unique name.
    pub(in crate::cli) fn release_fixture() -> (tempfile::TempDir, PathBuf, Task, Agent, TrackArgs) {
        fixture()
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, Task, Agent, TrackArgs) {
        let temp = tempfile::TempDir::new().unwrap();
        let repo = temp.path();
        git(repo, &["init", "-b", "main"]).unwrap();
        // CI runners have no global identity; annotated tags and merges need one.
        git(repo, &["config", "user.name", "Fixture"]).unwrap();
        git(repo, &["config", "user.email", "fixture@example.invalid"]).unwrap();
        git(
            repo,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "base",
            ],
        )
        .unwrap();
        let base = git(repo, &["rev-parse", "HEAD"]).unwrap();
        git(repo, &["update-ref", "refs/remotes/origin/main", &base]).unwrap();
        git(
            repo,
            &[
                "remote",
                "add",
                "origin",
                "https://github.com/Example/Repo.git",
            ],
        )
        .unwrap();
        let root = crate::store::init_cas_dir(repo).unwrap();
        fs::write(
            root.join("config.toml"),
            "[project]\ncanonical_id = \"release-fixture\"\n",
        )
        .unwrap();
        let mut train = Task::new("cas-a001".into(), "Train".into());
        train.task_type = TaskType::Epic;
        train.labels.push("release-train".into());
        train.branch = Some("epic/train-cas-a001".into());
        train.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:fixture".into(),
            target_branch: "main".into(),
        });
        let mut owner = Agent::new_with_role(
            "release-owner".into(),
            "owner".into(),
            AgentRole::Supervisor,
        );
        owner.factory_session = Some("original-session".into());
        train.epic_verification_owner = Some(owner.id.clone());
        open_agent_store(&root).unwrap().register(&owner).unwrap();
        open_task_store(&root).unwrap().add(&train).unwrap();
        git(repo, &["checkout", "-b", train.branch.as_deref().unwrap()]).unwrap();
        git(
            repo,
            &[
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "--allow-empty",
                "-m",
                "child work",
            ],
        )
        .unwrap();
        let sha = git(repo, &["rev-parse", "HEAD"]).unwrap();
        let args = TrackArgs {
            commit: sha.clone(),
            first_child: sha.clone(),
            version: "9.99.2".into(),
            inputs: None,
        };
        (temp, root, train, owner, args)
    }

    #[test]
    fn release_inventory_contract_roundtrip() {
        let (temp, _, train, _, args) = fixture();
        let mut value = serde_json::to_value(new_context(temp.path(), &train).unwrap()).unwrap();
        let scope = serde_json::json!({
            "project_id": "release-fixture", "repository": "Example/Repo",
            "release_id": "v9.99.2", "reviewed_sha": args.commit,
            "final_release_sha": args.commit, "promotion_pr": null, "release_tag": "v9.99.2",
            "constituent_epic_ids": [], "included_task_ids": [], "no_code_members": {}, "proof_refs": []
        });
        value["release_scope"] = scope.clone();
        let restored: ReleaseContext = serde_json::from_value(value).unwrap();
        assert_eq!(serde_json::to_value(restored).unwrap()["release_scope"], scope);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn explicit_no_code_proof_tracks_and_closes_once_cas_2ecc() {
        use crate::qa_batch::{ReleaseProofKind, ReleaseProofRef};
        use crate::mcp::tools::core::task::lifecycle::merge_close::CassyMergeClose;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let (temp, root, _, owner, mut args) = fixture();
        args.version = env!("CARGO_PKG_VERSION").into();
        let repo = temp.path();
        fs::write(root.join("config.toml"), format!(
            "[project]\ncanonical_id='release-fixture'\n[verification]\nenabled=false\n[factory]\nartifacts_root={:?}\n",
            root.join("artifacts").display().to_string(),
        )).unwrap();
        env.set("CAS_AGENT_ID", &owner.id);
        env.set("CAS_AGENT_NAME", &owner.name);
        env.set("CAS_AGENT_ROLE", "supervisor");
        let tasks = open_task_store(&root).unwrap();
        for id in ["cas-e3041", "cas-ef2b", "cas-undeclared"] {
            let mut task = Task::new(id.into(), "No-code report".into());
            task.origin_project = Some("release-fixture".into());
            task.execution_note = Some("no-code".into());
            task.status = TaskStatus::AwaitingMerge;
            task.risk = vec![cas_types::TaskRisk::None];
            tasks.add(&task).unwrap();
        }
        let path = root.join("artifacts/cas-e3041/report.md");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "# Report\nRecorded findings.\n").unwrap();
        let artifacts = cas_store::SqliteArtifactStore::open(&root).unwrap();
        let artifact = crate::artifacts::publish(&crate::artifacts::PublishContext {
            store: &artifacts,
            roots: crate::artifacts::paths::PublishRoots::new(&root, &root.join("artifacts"), "cas-e3041"),
            task_id: "cas-e3041".into(), cloud: None,
        }, &path).unwrap().artifact;
        for id in ["cas-e3041", "cas-ef2b", "cas-undeclared"] {
            let mut task = tasks.get(id).unwrap();
            task.external_ref = Some(artifact.id.clone());
            tasks.update(&task).unwrap();
        }
        let proof = ReleaseProofRef {
            kind: ReleaseProofKind::Report, artifact_id: artifact.id.clone(),
            sha256: artifact.sha256, size_bytes: artifact.size_bytes,
            reviewed_sha: args.commit.clone(), receipt_id: None,
        };
        let inputs = root.join("input-audit.json");
        fs::write(&inputs, serde_json::to_vec(&serde_json::json!({
            "checked_sha": args.commit, "included_task_ids": ["cas-e3041"],
            "no_code_members": {"cas-e3041": [proof]},
        })).unwrap()).unwrap();
        args.inputs = Some(inputs.clone());
        let train = tasks.get("cas-a001").unwrap();
        // A local publication is not portable proof until Cloud commits it.
        assert_eq!(artifact.status, "local");
        assert!(artifact.cloud_artifact_id.is_none());
        assert!(track_on_train(&args, &root, repo, &train).unwrap_err().to_string()
            .starts_with("RELEASE_NO_CODE_PROOF_BINDING"));
        assert!(!receipt_path(&root, &args.version).unwrap().exists());
        assert!(!crate::mcp::tools::core::task::release_train::is_cut_train(
            &tasks.get(&train.id).unwrap()));
        artifacts.mark_uploaded(&artifact.id, "cloud-report").unwrap();
        artifacts.mark_committed(&artifact.id, None).unwrap();
        let mut hash = proof.clone(); hash.sha256 = "0".repeat(64);
        let mut size = proof.clone(); size.size_bytes += 1;
        let mut missing = proof.clone(); missing.artifact_id = "art-absent".into();
        for (id, proofs) in [
            ("cas-e3041", vec![]), ("cas-e3041", vec![hash]),
            ("cas-e3041", vec![size]),
            ("cas-e3041", vec![missing]), ("cas-ef2b", vec![proof.clone()]),
        ] {
            fs::write(&inputs, serde_json::to_vec(&serde_json::json!({
                "checked_sha": args.commit, "included_task_ids": [id],
                "no_code_members": {id: proofs},
            })).unwrap()).unwrap();
            assert!(track_on_train(&args, &root, repo, &train).unwrap_err().to_string()
                .starts_with("RELEASE_NO_CODE_PROOF"));
            assert!(!receipt_path(&root, &args.version).unwrap().exists());
            assert!(!crate::mcp::tools::core::task::release_train::is_cut_train(
                &tasks.get(&train.id).unwrap()));
        }
        fs::write(&inputs, serde_json::to_vec(&serde_json::json!({
            "checked_sha": args.commit, "included_task_ids": ["cas-e3041"],
            "no_code_members": {"cas-e3041": [proof]},
        })).unwrap()).unwrap();
        let mut foreign = tasks.get("cas-e3041").unwrap();
        foreign.origin_project = Some("other-project".into());
        tasks.update(&foreign).unwrap();
        assert!(track_on_train(&args, &root, repo, &train).unwrap_err().to_string()
            .starts_with("RELEASE_NO_CODE_PROOF_BINDING"));
        assert!(!crate::mcp::tools::core::task::release_train::is_cut_train(
            &tasks.get(&train.id).unwrap()));
        foreign.origin_project = Some("release-fixture".into());
        tasks.update(&foreign).unwrap();
        track_on_train(&args, &root, repo, &train).unwrap();
        let context: ReleaseContext = serde_json::from_slice(
            &fs::read(receipt_path(&root, &args.version).unwrap()).unwrap(),
        ).unwrap();
        let tag = format!("v{}", args.version);
        git(repo, &["tag", &tag]).unwrap();
        let mut scope = super::super::release_adapters::tag_scope(
            &root, repo, Some(&context), &train.id, &tag, &args.commit,
        ).unwrap();
        assert_eq!(scope.no_code_members.get("cas-e3041"), Some(&vec![proof.clone()]));
        // A legacy publication may already contain an explicitly included,
        // unbound report. It remains a residual and cannot borrow external_ref.
        scope.included_task_ids.push("cas-ef2b".into());
        let inventory = crate::qa_batch::resolve_release_inventory(&root, repo, &scope).unwrap();
        assert!(!inventory.deliveries.iter().any(|d| d.task_id == "cas-undeclared"));
        assert!(!inventory.exceptions.iter().any(|d| d.task_id == "cas-undeclared"));
        let mut legacy = scope.clone();
        legacy.no_code_members.clear();
        let unbound = crate::qa_batch::resolve_release_inventory(&root, repo, &legacy).unwrap();
        let held = crate::release_reconcile::reconcile_release(
            &root, &legacy, &unbound, crate::release_reconcile::ReconcileTrigger::Publish { tag },
        ).await.unwrap();
        assert_eq!(tasks.get("cas-e3041").unwrap().status, TaskStatus::AwaitingMerge);
        assert!(held.residuals.iter().any(|r| r.task_id == "cas-e3041"));
        let mut undeclared = scope.clone();
        undeclared.no_code_members.insert("cas-undeclared".into(), vec![proof.clone()]);
        let extra = crate::qa_batch::resolve_release_inventory(&root, repo, &undeclared).unwrap();
        assert_eq!(crate::release_reconcile::reconcile_release(
            &root, &undeclared, &extra, crate::release_reconcile::ReconcileTrigger::Backfill,
        ).await.unwrap_err(), "RELEASE_IDENTITY_CHANGED");
        // Release close now requires an approval bound to the report's own
        // anchor, independently of the release-wide reviewed commit.
        cas_store::import_qa_batch_receipt(&root, &crate::qa_batch::QaBatchReceipt {
            schema_version: cas_store::RELEASE_VERDICT_SCHEMA, id: "qab-report-approved".into(),
            project_id: "release-fixture".into(), ledger_artifact_id: "art-review".into(),
            ledger_path: "/peer/report-review.md".into(), ledger_sha256: "b".repeat(64),
            ledger_size_bytes: 20, staging_sha: args.commit.clone(),
            task_heads: std::collections::BTreeMap::from([("cas-e3041".into(), args.commit.clone())]),
            recorder_agent_id: "supervisor".into(), recorded_at: chrono::Utc::now().to_rfc3339(),
            release_tag: None, ledger_author_id: Some("reviewer".into()),
            ledger_author_role: Some("sub_agent".into()), author_overrides: Default::default(), imported: true,
        }).unwrap();
        let first = crate::release_reconcile::reconcile_release(
            &root, &scope, &inventory, crate::release_reconcile::ReconcileTrigger::Backfill,
        ).await.unwrap();
        assert!(matches!(first.members.get("cas-e3041"), Some(CassyMergeClose::Closed { .. })), "{first:?}");
        assert!(first.residuals.iter().any(|r| r.task_id == "cas-ef2b" && r.gate.contains("MissingNoCodeProof")));
        let closed = tasks.get("cas-e3041").unwrap();
        assert_eq!(closed.status, TaskStatus::Closed);
        let replay_inventory = crate::qa_batch::resolve_release_inventory(&root, repo, &scope).unwrap();
        crate::release_reconcile::reconcile_release(
            &root, &scope, &replay_inventory, crate::release_reconcile::ReconcileTrigger::Backfill,
        ).await.unwrap();
        assert_eq!(tasks.get("cas-e3041").unwrap().closed_at, closed.closed_at);
        assert_eq!(tasks.get("cas-ef2b").unwrap().status, TaskStatus::AwaitingMerge);
    }

    #[test]
    fn release_inventory_resolves_nested_and_unclosed_structured_deliveries() {
        use crate::qa_batch::{
            ReleaseInventoryCode as Code, ReleaseMembership as Membership, ReleaseProofKind,
        };
        let (temp, root, train, _, _) = fixture();
        let repo = temp.path();
        let base = git(repo, &["rev-parse", "HEAD"]).unwrap();
        git(repo, &["checkout", "-b", "factory/inventory"]).unwrap();
        fs::write(repo.join("delivered.txt"), "delivered\n").unwrap();
        git(repo, &["add", "delivered.txt"]).unwrap();
        git(repo, &["commit", "-m", "delivery without a task ID"]).unwrap();
        let anchor = git(repo, &["rev-parse", "HEAD"]).unwrap();
        git(repo, &["checkout", "main"]).unwrap();
        git(repo, &["merge", "--squash", "factory/inventory"]).unwrap();
        git(repo, &["commit", "-m", "promotion"]).unwrap();
        let mapped = git(repo, &["rev-parse", "HEAD"]).unwrap();
        fs::write(repo.join("release.txt"), "release\n").unwrap();
        git(repo, &["add", "release.txt"]).unwrap();
        git(repo, &["commit", "-m", "release"]).unwrap();
        let final_sha = git(repo, &["rev-parse", "HEAD"]).unwrap();
        git(repo, &["tag", "v9.99.2"]).unwrap();
        let store = open_task_store(&root).unwrap();
        let mut nested = Task::new("cas-a002".into(), "Nested epic".into());
        nested.task_type = TaskType::Epic;
        nested.origin_project = Some("release-fixture".into());
        store.add(&nested).unwrap();
        let mut successor = Task::new("cas-a003".into(), "Successor train".into());
        successor.task_type = TaskType::Epic;
        successor.origin_project = Some("release-fixture".into());
        store.add(&successor).unwrap();
        // Fixture train predates project attribution; bind it explicitly.
        let mut train = train;
        train.origin_project = Some("release-fixture".into());
        store.update(&train).unwrap();
        store
            .add_dependency(&Dependency::new(
                nested.id.clone(),
                train.id.clone(),
                DependencyType::ParentChild,
            ))
            .unwrap();
        let mut code = Task::new("cas-b001".into(), "Delivery".into());
        code.origin_project = Some("release-fixture".into());
        code.status = TaskStatus::AwaitingMerge;
        code.branch = None; // real gabber incident: SQL branch is empty
        code.deliverables.factory_branch_anchor = Some(anchor.clone());
        code.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
            work_target: WorkTarget {
                repo_selector: "project:release-fixture".into(),
                target_branch: "main".into(),
            },
            branch: "factory/inventory".into(),
            assignee: "worker".into(),
            base_sha: base.clone(),
        });
        for (id, scenario) in [
            ("cas-b001", "nested"),
            ("cas-b002", "backlog"),
            ("cas-b003", "missing_base"),
            ("cas-b004", "retired"),
            ("cas-b005", "missing_anchor"),
            ("cas-b006", "foreign"),
            ("cas-b007", "no_code"),
            ("cas-b008", "no_code_missing"),
            ("cas-b009", "cancelled"),
            ("cas-b00a", "missing_mapping"),
            ("cas-b00b", "symbolic_anchor"),
            ("cas-b00c", "quarantined"),
            ("cas-b00d", "successor"),
            ("cas-b00e", "unstarted"),
            ("cas-b00f", "new_cycle"),
        ] {
            let mut t = code.clone();
            t.id = id.into();
            match scenario {
                "nested" => {
                    t.status = TaskStatus::Closed;
                }
                "missing_base" => {
                    t.deliverables.factory_branch_start = None;
                    t.deliverables.factory_branch_anchor = Some(final_sha.clone());
                }
                "retired" => {
                    t.deliverables.historical_factory_branch_anchors = t
                        .deliverables
                        .factory_branch_anchor
                        .take()
                        .into_iter()
                        .collect();
                }
                "missing_anchor" => {
                    t.deliverables.factory_branch_anchor = Some("f".repeat(40));
                }
                "symbolic_anchor" => {
                    t.deliverables.factory_branch_anchor = Some("main".into());
                }
                "foreign" => {
                    t.origin_project = Some("other-project".into());
                }
                "no_code" | "no_code_missing" => {
                    t.execution_note = Some("no-code".into());
                    t.deliverables = Default::default();
                }
                "cancelled" => {
                    t.status = TaskStatus::Cancelled;
                }
                "missing_mapping" => {
                    git(repo, &["checkout", "-b", "unshipped", &base]).unwrap();
                    fs::write(repo.join("absent.txt"), "absent\n").unwrap();
                    git(repo, &["add", "absent.txt"]).unwrap();
                    git(repo, &["commit", "-m", "unshipped"]).unwrap();
                    t.deliverables.factory_branch_anchor =
                        Some(git(repo, &["rev-parse", "HEAD"]).unwrap());
                }
                "successor" | "new_cycle" => {
                    t.deliverables.factory_branch_anchor = store.get("cas-b00a").unwrap()
                        .deliverables.factory_branch_anchor;
                    t.deliverables.factory_branch_start.as_mut().unwrap()
                        .work_target.target_branch = "epic/successor".into();
                    if scenario == "new_cycle" {
                        t.deliverables.historical_factory_branch_anchors.push(anchor.clone());
                    }
                }
                "unstarted" => {
                    t.status = TaskStatus::Open;
                    t.deliverables = Default::default();
                }
                _ => {}
            }
            store.add(&t).unwrap();
            if scenario == "quarantined" {
                let queue = crate::cloud::SyncQueue::open(&root).unwrap();
                queue.init().unwrap();
                queue
                    .quarantine_row(crate::cloud::QUARANTINE_TASK, id, "legacy marker")
                    .unwrap();
            }
            if scenario != "backlog" {
                store
                    .add_dependency(&Dependency::new(
                        id.into(),
                        if scenario == "successor" { successor.id.clone() } else { nested.id.clone() },
                        DependencyType::ParentChild,
                    ))
                    .unwrap();
            }
        }
        for (schema, id) in [
            (cas_store::QA_BATCH_RECEIPT_SCHEMA, "qa"),
            (cas_store::RELEASE_VERDICT_SCHEMA, "review"),
            (cas_store::RELEASE_TAG_VERDICT_SCHEMA, "tag"),
        ] {
            let is_tag = schema == cas_store::RELEASE_TAG_VERDICT_SCHEMA;
            cas_store::record_qa_batch_receipt(
                &root,
                &cas_store::QaBatchReceipt {
                    schema_version: schema,
                    id: id.into(),
                    project_id: "release-fixture".into(),
                    ledger_artifact_id: if is_tag {
                        String::new()
                    } else {
                        format!("art-{id}")
                    },
                    ledger_path: String::new(),
                    ledger_sha256: if is_tag {
                        String::new()
                    } else {
                        "a".repeat(64)
                    },
                    ledger_size_bytes: if is_tag { 0 } else { 10 },
                    staging_sha: anchor.clone(),
                    task_heads: [(code.id.clone(), anchor.clone())].into(),
                    recorder_agent_id: "reviewer".into(),
                    recorded_at: chrono::Utc::now().to_rfc3339(),
                    release_tag: is_tag.then(|| "v9.99.2".into()),
                    ledger_author_id: None,
                    ledger_author_role: None,
                    author_overrides: Default::default(),
                    imported: false,
                },
            )
            .unwrap();
        }
        let scope = crate::qa_batch::ReleaseScope {
            project_id: "release-fixture".into(),
            repository: "Example/Repo".into(),
            release_id: "v9.99.2".into(),
            reviewed_sha: anchor.clone(),
            final_release_sha: final_sha.clone(),
            promotion_pr: Some(4396),
            release_tag: Some("v9.99.2".into()),
            constituent_epic_ids: vec![train.id],
            // These deliberately broken members are explicit obligations.
            // Descendant discovery alone must not declare missing work shipped.
            included_task_ids: ["cas-b004", "cas-b005", "cas-b006", "cas-b008",
                "cas-b009", "cas-b00a", "cas-b00b", "cas-missing"]
                .into_iter().map(str::to_string).collect(),
            proof_refs: Vec::new(),
            no_code_members: [(
                "cas-b007".into(),
                vec![crate::qa_batch::ReleaseProofRef {
                    kind: ReleaseProofKind::Report,
                    artifact_id: "art-report".into(),
                    sha256: "a".repeat(64),
                    size_bytes: 10,
                    reviewed_sha: anchor.clone(),
                    receipt_id: None,
                }],
            )]
            .into(),
        };
        let inventory = crate::qa_batch::resolve_release_inventory(&root, repo, &scope).unwrap();
        let ids: Vec<_> = inventory
            .deliveries
            .iter()
            .map(|d| d.task_id.as_str())
            .collect();
        assert_eq!(
            ids,
            ["cas-b001", "cas-b002", "cas-b003", "cas-b007", "cas-b00c"]
        );
        let shipped = &inventory.deliveries[0];
        assert_eq!(shipped.anchor.as_deref(), Some(anchor.as_str()));
        assert_eq!(shipped.base_sha.as_deref(), Some(base.as_str()));
        assert_eq!(shipped.shipped_commit.as_deref(), Some(mapped.as_str()));
        assert_eq!(shipped.membership, Membership::SquashTree);
        assert_eq!(
            shipped.generation,
            store.get(&code.id).unwrap().updated_at.to_rfc3339()
        );
        assert_eq!(shipped.proof_refs.len(), 2);
        assert!(
            shipped
                .proof_refs
                .iter()
                .any(|p| p.kind == ReleaseProofKind::Qa && p.artifact_id == "art-qa")
        );
        assert!(
            shipped
                .proof_refs
                .iter()
                .any(|p| p.kind == ReleaseProofKind::Review && p.artifact_id == "art-review")
        );
        assert_eq!(inventory.deliveries[2].membership, Membership::Ancestry);
        assert_eq!(
            inventory.deliveries[3].membership,
            Membership::ExplicitNoCode
        );
        let errors: Vec<_> = inventory
            .exceptions
            .iter()
            .map(|e| (e.task_id.as_str(), e.code.clone()))
            .collect();
        assert_eq!(
            errors,
            [
                ("cas-b003", Code::MissingBase),
                ("cas-b004", Code::RetiredAnchor),
                ("cas-b005", Code::MissingAnchor),
                ("cas-b006", Code::ForeignScope),
                ("cas-b008", Code::MissingNoCodeProof),
                ("cas-b009", Code::Cancelled),
                ("cas-b00a", Code::MissingMapping),
                ("cas-b00b", Code::MissingAnchor),
                ("cas-b00c", Code::Quarantined),
                ("cas-b00f", Code::MissingMapping),
                ("cas-missing", Code::MissingTask)
            ]
        );
        let future = store.get("cas-b00d").unwrap();
        assert_eq!(future.status, TaskStatus::AwaitingMerge);
        assert_eq!(future.deliverables.factory_branch_start.unwrap().work_target.target_branch,
            "epic/successor");
        let unstarted = store.get("cas-b00e").unwrap();
        assert_eq!(unstarted.status, TaskStatus::Open);
        assert!(unstarted.deliverables.factory_branch_start.is_none());
        let new_cycle = store.get("cas-b00f").unwrap();
        assert_eq!(new_cycle.deliverables.factory_branch_anchor,
            store.get("cas-b00a").unwrap().deliverables.factory_branch_anchor);
        assert_eq!(new_cycle.deliverables.historical_factory_branch_anchors, [anchor.clone()]);
        assert_eq!(
            git(repo, &["rev-parse", "HEAD"]).unwrap(),
            store
                .get("cas-b00a")
                .unwrap()
                .deliverables
                .factory_branch_anchor
                .unwrap()
        );
        assert!(
            !open_task_store(&root)
                .unwrap()
                .list(None)
                .unwrap()
                .iter()
                .any(|t| t.id == "cas-b00c")
        );
        let mut wrong = scope.clone();
        wrong.project_id = "other".into();
        assert!(crate::qa_batch::resolve_release_inventory(&root, repo, &wrong).is_err());
        wrong = scope;
        wrong.final_release_sha = mapped;
        assert!(crate::qa_batch::resolve_release_inventory(&root, repo, &wrong).is_err());
    }

    #[test]
    fn release_tracking_and_installed_notice_are_restart_safe() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let (temp, root, mut train, owner, mut args) = fixture();
        // A pending notice must remain deliverable after the binary upgrades.
        args.version = "0.1.0".into();
        let repo = temp.path();
        let sha = args.commit.clone();
        let audit = serde_json::json!({"checked_sha": sha, "epic_refs": ["origin/epic/constituent"],
            "deferred_tasks": {"cas-0002": "next release"}, "included_task_ids": ["cas-0001"]});
        let audit_path = root.join("input-audit.json");
        fs::write(&audit_path, serde_json::to_vec(&audit).unwrap()).unwrap();
        args.inputs = Some(audit_path);
        track_on_train(&args, &root, repo, &train).unwrap();
        track_on_train(&args, &root, repo, &train).unwrap();
        let reminders = open_reminder_store(&root).unwrap();
        let pending = reminders.list_all_pending().unwrap();
        assert_eq!(pending.len(), 2);
        for reminder in &pending {
            assert!(reminder.cross_session);
            assert_eq!(reminder.task_id.as_deref(), Some(train.id.as_str()));
            assert_eq!(reminder.owner_id, owner.id);
            reminders.mark_fired(reminder.id, None).unwrap();
        }
        track_on_train(&args, &root, repo, &train).unwrap();
        assert!(reminders.list_all_pending().unwrap().is_empty());
        let recorded: ReleaseContext =
            serde_json::from_slice(&fs::read(receipt_path(&root, &args.version).unwrap()).unwrap())
                .unwrap();
        assert_eq!(recorded.release_inputs.map(|i| serde_json::to_value(i).unwrap()), Some(audit));
        assert_eq!(recorded.first_child_commit.as_deref(), Some(sha.as_str()));
        assert_eq!(
            recorded.first_child_epoch,
            Some(
                git(repo, &["show", "-s", "--format=%ct", &sha])
                    .unwrap()
                    .parse()
                    .unwrap()
            )
        );
        // Installing reconciles the train from its published tag, so publish it.
        let bare = repo.join("origin.git");
        git(repo, &["init", "--bare", bare.to_str().unwrap()]).unwrap();
        // Keep the GitHub identity the release scope needs; transport goes local.
        git(repo, &["config", &format!("url.{}.insteadOf", bare.display()), "https://github.com/Example/Repo.git"]).unwrap();
        let tag = format!("v{}", args.version);
        git(repo, &["tag", "-a", &tag, "-m", "release", &sha]).unwrap();
        git(repo, &["push", "origin", train.branch.as_deref().unwrap(), &tag]).unwrap();
        // Core may roll to the next train before the host installs.
        train.status = cas_types::TaskStatus::Closed;
        train.epic_verification_owner = None;
        open_task_store(&root).unwrap().update(&train).unwrap();
        assert!(!installed_at_root(&root, "example/other", &args.version).unwrap());
        let publication_path = root.join("publication.json");
        fs::write(&publication_path, serde_json::to_vec(&serde_json::json!({
            "tag_name": tag, "draft": false, "published_at": "2026-11-05T23:00:00Z",
        })).unwrap()).unwrap();
        let before = open_task_store(&root).unwrap().get(&train.id).unwrap();
        for _ in 0..2 {
            installed(&InstalledArgs {
                receipt: publication_path.clone(), repo: "example/repo".into(), failed_attempts: None,
            }, Some(&root)).unwrap();
        }
        let after = open_task_store(&root).unwrap().get(&train.id).unwrap();
        assert_eq!(after.status, TaskStatus::Closed);
        assert_eq!(after.closed_at, before.closed_at);
        assert_eq!(after.updated_at, before.updated_at);
        let rows = crate::store::open_prompt_queue_store(&root)
            .unwrap()
            .peek_all(10)
            .unwrap();
        // One post-release notice and one closeout notice from the shared operation.
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|row| row.target == owner.name));
        assert!(rows.iter().any(|row| row.prompt.contains("kind=\"release_closeout\"")));
        assert!(crate::supervisor_notices::is_action_envelope(
            &rows[0].prompt
        ));
        fs::write(&publication_path, serde_json::to_vec(&serde_json::json!({
            "tag_name": "v999.0.0", "draft": false, "published_at": "2026-11-05T23:00:00Z",
        })).unwrap()).unwrap();
        assert!(installed(&InstalledArgs {
            receipt: publication_path, repo: "example/repo".into(), failed_attempts: None,
        }, Some(&root)).is_err());
    }

    #[test]
    fn failed_install_notice_reaches_owner_once_cas_fed1() {
        use clap::Parser;
        let _env = crate::test_support::TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let (temp, root, mut train, owner, mut tracking) = fixture();
        tracking.version = "0.1.0".into();
        track_on_train(&tracking, &root, temp.path(), &train).unwrap();
        // The receipt retains ownership after the release train has rolled over.
        train.status = TaskStatus::Closed;
        train.epic_verification_owner = None;
        open_task_store(&root).unwrap().update(&train).unwrap();
        let publication = root.join("failed-publication.json");
        let before = open_task_store(&root).unwrap().get(&train.id).unwrap();
        fs::write(&publication, r#"{"tag_name":"v0.1.0","draft":false,"published_at":"2026-11-05T23:00:00Z"}"#).unwrap();
        let cli = super::super::Cli::try_parse_from([
            "cas", "release", "installed", "--receipt", publication.to_str().unwrap(),
            "--repo", "example/repo", "--failed-attempts", "3",
        ]).unwrap();
        let Some(super::super::Commands::Release(super::super::ReleaseCommands::Installed(args))) = cli.command else { panic!("wrong command"); };
        for _ in 0..2 { installed(&args, Some(&root)).unwrap(); }
        let rows = crate::store::open_prompt_queue_store(&root).unwrap().peek_all(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, owner.name);
        assert!(crate::supervisor_notices::is_action_envelope(&rows[0].prompt));
        let notices = crate::store::open_supervisor_queue_store(&root).unwrap()
            .get_by_transition_key(&format!("supervisor-notice:release_post_publish_failed:{}:0.1.0:{}", train.id, owner.id)).unwrap();
        assert!(notices.is_some());
        // There is deliberately no published tag or Git transport for reconciliation.
        assert_eq!(open_task_store(&root).unwrap().get(&train.id).unwrap().updated_at, before.updated_at);
    }

    #[test]
    fn release_git_child_has_a_deadline() {
        let temp = tempfile::tempdir().unwrap();
        git(temp.path(), &["init"]).unwrap();
        assert!(git_with_timeout(
            temp.path(), &["-c", "alias.hang=!exec /bin/sleep 1", "hang"],
            std::time::Duration::from_millis(20),
        ).is_err());
    }

    #[tokio::test]
    async fn a_cut_train_sends_new_tasks_to_a_fresh_train_cas_2b8d() {
        use rmcp::handler::server::wrapper::Parameters;
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        crate::store::known_repos::ensure_host_schema().unwrap();
        let (temp, root, train, _, args) = fixture();
        // Trains are opt-in (GH #1613); this case covers the next train.
        fs::write(root.join("config.toml"),
            "[project]\ncanonical_id = \"release-fixture\"\n\n[factory]\nrelease_train = true\n").unwrap();
        let _cwd = env.enter_dir(temp.path());
        track(&args, &root).unwrap();
        let core = crate::mcp::CasCore::with_daemon(root.clone(), None, None);
        core.cas_task_create(Parameters(serde_json::from_value(serde_json::json!({
            "title": "Next train followup", "task_type": "task", "risk": "none",
        })).unwrap())).await.unwrap();
        let store = open_task_store(&root).unwrap();
        let child = store.list(None).unwrap().into_iter().find(|t| t.title == "Next train followup").unwrap();
        let next = store.get_parent_epic(&child.id).unwrap().unwrap();
        assert_ne!(next.id, train.id, "new work must not join the queued release");
        assert_eq!(git(temp.path(), &["rev-parse", next.effective_epic_branch().unwrap()]).unwrap(),
            git(temp.path(), &["rev-parse", "main"]).unwrap(), "fresh train starts from integration");
        assert!(store.get(&train.id).unwrap().labels.iter().any(|label| label == "release-train-cut"));
        // A restarted cut still resolves its original train, even after new
        // work has opened a different train. Its reminders stay idempotent.
        track(&args, &root).unwrap();
        assert_eq!(open_reminder_store(&root).unwrap().list_all_pending().unwrap().len(), 2);
        let recorded: ReleaseContext = serde_json::from_slice(&fs::read(receipt_path(&root, &args.version).unwrap()).unwrap()).unwrap();
        assert_eq!(recorded.train_id, train.id);
    }
    #[test]
    fn release_tracking_owner_fallback_never_blocks() {
        for (recorded_owner, peer_available, reminder_error) in [(false, true, false), (false, false, false), (true, false, false), (true, true, true)] {
            let (temp, root, mut train, owner, args) = fixture();
            if !recorded_owner {
                train.epic_verification_owner = None;
            }
            if !peer_available {
                open_agent_store(&root)
                    .unwrap()
                    .unregister(&owner.id)
                    .unwrap();
            }
            if reminder_error {
                open_reminder_store(&root).unwrap();
                rusqlite::Connection::open(root.join("cas.db")).unwrap().execute_batch(
                    "CREATE TRIGGER deny_reminder BEFORE INSERT ON reminders BEGIN SELECT RAISE(ABORT, 'fixture reminder unavailable'); END;"
                ).unwrap();
            }
            track_on_train(&args, &root, temp.path(), &train).unwrap();
            let pending = open_reminder_store(&root)
                .unwrap()
                .list_all_pending()
                .unwrap();
            assert_eq!(pending.len(), if peer_available && !reminder_error { 2 } else { 0 });
            assert!(open_task_store(&root).unwrap().get(&train.id).unwrap().labels.iter().any(|label| label == "release-train-cut"));
        }
    }

    #[test]
    fn release_version_rebump_cancels_both_old_watches() {
        let (temp, root, train, owner, mut args) = fixture();
        track_on_train(&args, &root, temp.path(), &train).unwrap();
        let reminders = open_reminder_store(&root).unwrap();
        let old = reminders.list_all_pending().unwrap();
        args.version = "9.99.3".into();
        track_on_train(&args, &root, temp.path(), &train).unwrap();
        let pending = reminders.list_all_pending().unwrap();
        assert_eq!(pending.len(), 2);
        assert!(
            pending
                .iter()
                .all(|r| !old.iter().any(|old| old.id == r.id))
        );
        assert_eq!(
            reminders.list_recently_ended(&owner.id, 60).unwrap().len(),
            2
        );
    }

    #[test]
    fn published_train_backfill_preview_names_every_child_without_writes() {
        let (temp, root, mut shipped, owner, _) = fixture();
        let repo = temp.path();
        let old_lane = shipped.branch.clone().unwrap();
        let next_lane = "epic/release-train-next001";
        fs::write(root.join("config.toml"),
            "[project]\ncanonical_id = \"release-fixture\"\n\n[factory]\nepic_base_branch = \"main\"\nrelease_train = true\n\n[qa]\nmode = \"batch\"\n").unwrap();
        let bare = temp.path().join("origin.git");
        git(repo, &["init", "--bare", bare.to_str().unwrap()]).unwrap();
        git(repo, &["config", &format!("url.{}.insteadOf", bare.display()), "https://github.com/Example/Repo.git"]).unwrap();
        git(repo, &["remote", "set-url", "origin", "https://github.com/Example/Repo.git"]).unwrap();
        fs::create_dir_all(repo.join("src")).unwrap();
        fs::write(repo.join("src/released.rs"), "pub fn released() {}\n").unwrap();
        git(repo, &["add", "src/released.rs"]).unwrap();
        git(repo, &["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-m", "delivery"])
            .unwrap();
        let anchor = git(repo, &["rev-parse", "HEAD"]).unwrap();
        git(repo, &["checkout", "main"]).unwrap();
        git(repo, &["merge", "--no-ff", &old_lane, "-m", "merge release train"]).unwrap();
        fs::write(repo.join("src/released.rs"), "pub fn released() { /* shipped edit */ }\n").unwrap();
        git(repo, &["add", "src/released.rs"]).unwrap();
        git(repo, &["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-m", "edit shipped file after merge"])
            .unwrap();
        fs::write(repo.join("CHANGELOG.md"), "# Changelog\n\n## [Unreleased]\n\n## [4.3.0] - 2026-11-08\n\n- Shipped.\n").unwrap();
        git(repo, &["add", "CHANGELOG.md"]).unwrap();
        git(repo, &["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-m", "release: v4.3.0"])
            .unwrap();
        git(repo, &["tag", "-a", "v4.3.0", "-m", "release v4.3.0"]).unwrap();
        git(repo, &["branch", next_lane, "main"]).unwrap();
        git(repo, &["push", "origin", "main", &old_lane, next_lane, "--tags"]).unwrap();
        // This tag exists only locally. Publication checks must reject it.
        git(repo, &["tag", "-a", "v4.3.1", "-m", "local-only release"]).unwrap();
        let release_sha = git(repo, &["rev-parse", "v4.3.0^{commit}"]).unwrap();
        let reused_branch = "factory/reused-shipped-lane";
        git(repo, &["checkout", "-b", reused_branch, "v4.3.0"]).unwrap();
        git(repo, &["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-m", "post-tag next-train work"])
            .unwrap();
        git(repo, &["checkout", "main"]).unwrap();
        let unmerged_branch = "factory/unmerged-delivery";
        git(repo, &["checkout", "-b", unmerged_branch, "v4.3.0"]).unwrap();
        for message in ["unmerged one", "unmerged two"] {
            git(repo, &["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-m", message]).unwrap();
        }
        git(repo, &["push", "origin", unmerged_branch]).unwrap();
        git(repo, &["checkout", "main"]).unwrap();

        let store = open_task_store(&root).unwrap();
        shipped.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:release-fixture".into(),
            target_branch: "main".into(),
        });
        store.update(&shipped).unwrap();
        let mut current = Task::new("cas-next001".into(), "Current train".into());
        current.task_type = TaskType::Epic;
        current.labels.push("release-train".into());
        current.branch = Some(next_lane.into());
        current.deliverables.work_target = shipped.deliverables.work_target.clone();
        store.add(&current).unwrap();

        let mut shipped_child = Task::new("cas-a100".into(), "GH #101 GH #102 GH #103 GH #104 GH #105: shipped fix".into());
        shipped_child.origin_project = Some("release-fixture".into());
        shipped_child.assignee = Some(owner.id.clone());
        shipped_child.status = TaskStatus::AwaitingMerge;
        shipped_child.deliverables.factory_branch_anchor = Some(anchor);
        let shipped_target = WorkTarget {
            repo_selector: "project:release-fixture".into(),
            target_branch: old_lane.clone(),
        };
        shipped_child.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
            work_target: shipped_target.clone(),
            assignee: owner.id.clone(),
            branch: reused_branch.into(),
            base_sha: release_sha.clone(),
        });
        shipped_child.deliverables.work_target = Some(shipped_target);
        assert_eq!(unmerged_factory_commit_count(repo, &shipped_child, &release_sha), 1);
        store.create_atomic(&shipped_child, &[], Some(&shipped.id), None).unwrap();
        let mut unmerged_child = Task::new("cas-a102".into(), "GH #102: remaining part".into());
        unmerged_child.origin_project = Some("release-fixture".into());
        let unmerged_target = WorkTarget {
            repo_selector: "project:release-fixture".into(),
            target_branch: old_lane.clone(),
        };
        unmerged_child.deliverables.work_target = Some(unmerged_target.clone());
        unmerged_child.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
            work_target: unmerged_target,
            assignee: owner.id.clone(),
            branch: unmerged_branch.into(),
            base_sha: release_sha,
        });
        store.create_atomic(&unmerged_child, &[], Some(&shipped.id), None).unwrap();
        let mut another_open_child = Task::new("cas-a101".into(), "Move to successor".into());
        another_open_child.origin_project = Some("release-fixture".into());
        another_open_child.deliverables.work_target = Some(WorkTarget {
            repo_selector: "project:release-fixture".into(),
            target_branch: old_lane.clone(),
        });
        store.create_atomic(&another_open_child, &[], Some(&shipped.id), None).unwrap();

        let mut prior = Task::new("cas-prior".into(), "GH #104: older released part".into());
        prior.origin_project = Some("release-fixture".into());
        prior.status = TaskStatus::Closed;
        prior.deliverables.factory_branch_anchor = shipped_child.deliverables.factory_branch_anchor.clone();
        store.add(&prior).unwrap();
        let hidden = Task::new("cas-hidden".into(), "GH #105: unfinished hidden part".into());
        store.add(&hidden).unwrap();
        let queue = crate::cloud::SyncQueue::open(&root).unwrap();
        queue.init().unwrap();
        queue.quarantine_row(crate::cloud::QUARANTINE_TASK, &hidden.id, "fixture hidden reference").unwrap();
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_SESSION_ID", &owner.id);
        env.set("CAS_AGENT_NAME", &owner.name);
        env.set("CAS_AGENT_ROLE", "supervisor");
        let gh = repo.join("fixture-gh.py");
        fs::write(&gh, r#"#!/usr/bin/env python3
import json, pathlib, sys
root = pathlib.Path(__file__).parent
args = sys.argv[1:]
with (root / 'gh-calls.jsonl').open('a') as log:
    log.write(json.dumps(args) + '\n')
if (root / 'gh-offline').exists():
    print('fixture offline', file=sys.stderr)
    sys.exit(1)
if args[:2] == ['issue', 'list']:
    print(json.dumps([{'number': n} for n in [101, 102, 103, 104, 105] if not (root / f'closed-{n}').exists()]))
elif args[:2] == ['issue', 'close']:
    if args[2] == '103':
        print('fixture transport failure', file=sys.stderr)
        sys.exit(1)
    (root / f'closed-{args[2]}').touch()
else:
    sys.exit(2)
"#).unwrap();
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
        }
        env.set(crate::github_issue_attach::GH_BIN_ENV, &gh);
        let _cwd = env.enter_dir(repo);
        assert!(preview_shipped_train(&root, &shipped.id, "v4.3.1").is_err());
        assert_eq!(store.get(&shipped.id).unwrap().status, TaskStatus::Open);
        assert_eq!(store.get_parent_epic(&unmerged_child.id).unwrap().unwrap().id, shipped.id);
        assert_eq!(store.get(&unmerged_child.id).unwrap().deliverables.work_target.unwrap().target_branch, old_lane);
        let preview = preview_shipped_train(&root, &shipped.id, "v4.3.0").unwrap();
        assert!(preview.contains("cas-a102: HOLD (not in tag; 2 unmerged commits)"), "{preview}");
        assert!(preview.contains("cas-a101: HOLD (not in tag; 0 unmerged commits)"), "{preview}");
        assert!(preview.contains("no writes"), "{preview}");
        let tag_receipt = crate::qa_batch::prepare_release_tag_receipt(
            &root,
            std::slice::from_ref(&shipped_child.id),
            "v4.3.0",
        ).unwrap();
        assert_eq!(tag_receipt.task_heads.get(&shipped_child.id), shipped_child.deliverables.factory_branch_anchor.as_ref());
        assert_eq!(store.get(&shipped.id).unwrap().status, TaskStatus::Open);
        assert_eq!(store.get_parent_epic(&unmerged_child.id).unwrap().unwrap().id, shipped.id);
        assert_eq!(store.get(&unmerged_child.id).unwrap().deliverables.work_target.unwrap().target_branch, old_lane);
        assert_eq!(store.get_parent_epic(&another_open_child.id).unwrap().unwrap().id, shipped.id);
        assert_eq!(store.get(&another_open_child.id).unwrap().deliverables.work_target.unwrap().target_branch, old_lane);
        assert!(cas_store::release_verdicts_for_task(&root, "release-fixture", &shipped_child.id).unwrap().is_empty());
    }

    fn published_backfill_fixture() -> (tempfile::TempDir, PathBuf, Task, Task) {
        let (temp, root, mut train, _, _) = fixture();
        let repo = temp.path();
        fs::write(root.join("config.toml"),
            "[project]\ncanonical_id = 'release-fixture'\n[factory]\nepic_base_branch = 'main'\nrelease_train = true\n[qa]\nmode = 'batch'\n").unwrap();
        let bare = repo.join("origin.git");
        git(repo, &["init", "--bare", bare.to_str().unwrap()]).unwrap();
        git(repo, &["remote", "set-url", "origin", bare.to_str().unwrap()]).unwrap();
        git(repo, &["tag", "v4.3.0"]).unwrap();
        git(repo, &["push", "origin", "main", train.branch.as_deref().unwrap(), "--tags"]).unwrap();
        train.labels.push("release-train-cut".into());
        train.deliverables.work_target.as_mut().unwrap().repo_selector = "project:release-fixture".into();
        let store = open_task_store(&root).unwrap();
        store.update(&train).unwrap();
        let mut successor = Task::new("cas-next001".into(), "Next train".into());
        successor.task_type = TaskType::Epic;
        successor.labels.push("release-train".into());
        successor.branch = Some("epic/release-train-next001".into());
        successor.deliverables.work_target = train.deliverables.work_target.clone();
        store.add(&successor).unwrap();
        (temp, root, train, successor)
    }

    #[test]
    fn backfill_preview_checks_release_close_admission_cas_e8cc() {
        let (temp, root, train, _) = published_backfill_fixture();
        let repo = temp.path();
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let _cwd = env.enter_dir(repo);
        let store = open_task_store(&root).unwrap();
        let mut child = Task::new("cas-a100".into(), "Shipped dedicated review".into());
        child.origin_project = Some("release-fixture".into());
        child.status = TaskStatus::AwaitingMerge;
        child.deliverables.factory_branch_anchor = Some(git(repo, &["rev-parse", "HEAD"]).unwrap());
        child.deliverables.work_target = train.deliverables.work_target.clone();
        store.create_atomic(&child, &[], Some(&train.id), None).unwrap();
        for gate in ["own-review", "ci-override", "request-changes"] {
            let mut held = child.clone();
            if gate == "ci-override" {
                held.deliverables.merge_ci_admission = Some(cas_types::MergeCiAdmission::Override);
            } else if gate == "request-changes" {
                held.status = TaskStatus::Open;
                held.deliverables.historical_factory_branch_anchors = held.deliverables.factory_branch_anchor.take().into_iter().collect();
            }
            store.update(&held).unwrap();
            preview_shipped_train(&root, &train.id, "v4.3.0").unwrap();
            assert!(preview_release_close_hold(&root, &held, repo).is_some(), "{gate}");
            let after = store.get(&held.id).unwrap();
            assert_eq!(after.status, held.status);
            assert_eq!(after.deliverables.factory_branch_anchor, held.deliverables.factory_branch_anchor);
            assert_eq!(store.get_parent_epic(&held.id).unwrap().unwrap().id, train.id);
            assert_eq!(store.get(&train.id).unwrap().status, TaskStatus::Open);
            assert!(cas_store::release_verdicts_for_task(&root, "release-fixture", &held.id).unwrap().is_empty());
        }
    }

    #[test]
    fn backfill_uses_later_published_tag_for_shipped_child_cas_e8cc() {
        let (temp, root, train, _) = published_backfill_fixture();
        let repo = temp.path();
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let _cwd = env.enter_dir(repo);
        let base = git(repo, &["rev-parse", "HEAD"]).unwrap();
        git(repo, &["checkout", "-b", "factory/later-shipped"]).unwrap();
        fs::write(repo.join("later.txt"), "later release delivery\n").unwrap();
        git(repo, &["add", "later.txt"]).unwrap();
        git(repo, &["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "-m", "fix(cas-later001): later release delivery"]).unwrap();
        let anchor = git(repo, &["rev-parse", "HEAD"]).unwrap();
        git(repo, &["tag", "v4.4.0"]).unwrap();
        git(repo, &["push", "origin", "refs/tags/v4.4.0"]).unwrap();
        git(repo, &["-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-m", "another task reuses this branch"]).unwrap();
        let mut child = Task::new("cas-later001".into(), "Shipped by later release".into());
        child.origin_project = Some("release-fixture".into());
        child.status = TaskStatus::AwaitingMerge;
        child.assignee = Some("release-owner".into());
        child.deliverables.factory_branch_anchor = Some(anchor.clone());
        child.deliverables.work_target = train.deliverables.work_target.clone();
        child.deliverables.factory_branch_start = Some(cas_types::FactoryBranchStart {
            work_target: child.deliverables.work_target.clone().unwrap(),
            assignee: "release-owner".into(), branch: "factory/later-shipped".into(), base_sha: base,
        });
        child.deliverables.merge_ci_admission = Some(cas_types::MergeCiAdmission::Override);
        let store = open_task_store(&root).unwrap();
        store.create_atomic(&child, &[], Some(&train.id), None).unwrap();
        let preview = preview_shipped_train(&root, &train.id, "v4.3.0").unwrap();
        let line = preview.lines().find(|line| line.contains("cas-later001: ")).unwrap();
        assert!(!line.contains("not in tag"), "{preview}");
        let receipt = crate::qa_batch::prepare_release_tag_receipt(&root, std::slice::from_ref(&child.id), "v4.4.0").unwrap();
        assert_eq!(receipt.task_heads.get(&child.id), Some(&anchor));
        assert_eq!(store.get_parent_epic(&child.id).unwrap().unwrap().id, train.id);
        assert_eq!(store.get(&child.id).unwrap().status, TaskStatus::AwaitingMerge);
    }

    #[test]
    fn backfill_continues_after_bad_train_and_accepts_legacy_train_lane() {
        let (temp, root, _, _, _) = fixture();
        let repo = temp.path();
        let bare = temp.path().join("origin.git");
        fs::write(
            root.join("config.toml"),
            "[project]\ncanonical_id = \"release-fixture\"\n\n[factory]\nepic_base_branch = \"main\"\nrelease_train = true\n",
        ).unwrap();
        git(repo, &["init", "--bare", bare.to_str().unwrap()]).unwrap();
        git(repo, &["remote", "set-url", "origin", bare.to_str().unwrap()]).unwrap();
        let store = open_task_store(&root).unwrap();
        let target = WorkTarget {
            repo_selector: "project:release-fixture".into(),
            target_branch: "main".into(),
        };
        let mut current = Task::new("cas-51aa9".into(), "Current train".into());
        current.task_type = TaskType::Epic;
        current.labels.push("release-train".into());
        current.branch = Some("epic/release-train-vnext-cas-51aa9".into());
        current.deliverables.work_target = Some(target.clone());
        store.add(&current).unwrap();

        let trains = [
            ("cas-c4a0", "v4.3.0"),
            ("cas-baf5", "v4.4.0"),
            ("cas-dcb3", "v4.5.0"),
            ("cas-d7c7", "v4.6.0"),
        ];
        let main = git(repo, &["rev-parse", "main"]).unwrap();
        for (id, tag) in trains {
            let lane = if id == "cas-d7c7" {
                "epic/release-train-v4-5-1-cas-d7c7".to_string()
            } else {
                format!("epic/release-train-{id}")
            };
            git(repo, &["branch", &lane, &main]).unwrap();
            let mut train = Task::new(id.into(), format!("Release train {id}").into());
            train.task_type = if id == "cas-c4a0" { TaskType::Task } else { TaskType::Epic };
            if id != "cas-d7c7" {
                train.labels.push("release-train".into());
            }
            train.branch = Some(lane);
            train.deliverables.work_target = Some(target.clone());
            store.add(&train).unwrap();
            git(repo, &["tag", "-a", tag, "-m", tag]).unwrap();
        }
        let mut push = vec!["push", "origin", "main"];
        let lanes = trains.iter().map(|(id, _)| {
            if *id == "cas-d7c7" {
                "epic/release-train-v4-5-1-cas-d7c7".to_string()
            } else {
                format!("epic/release-train-{id}")
            }
        }).collect::<Vec<_>>();
        push.extend(lanes.iter().map(String::as_str));
        push.push("--tags");
        git(repo, &push).unwrap();

        let mut env = crate::test_support::TestEnvGuard::temp_home();
        let _cwd = env.enter_dir(repo);
        assert!(backfill(&BackfillArgs { dry_run: true }, &root).is_err());
        for (id, _) in trains {
            assert_eq!(store.get(id).unwrap().status, TaskStatus::Open, "dry-run mutated {id}");
        }
        // Backfill never writes a train's status: each finished train is held
        // for the shared release reconciliation (cas-c1db), which this
        // non-GitHub fixture cannot resolve, so every train stays open.
        for _ in 0..2 {
            assert!(backfill(&BackfillArgs { dry_run: false }, &root).is_err());
            for (id, _) in trains {
                assert_eq!(store.get(id).unwrap().status, TaskStatus::Open, "{id}");
            }
        }
    }
}
