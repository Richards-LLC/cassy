//! Release script adapters. Train selection and notice delivery stay in core.
use crate::store::{open_agent_store, open_reminder_store, open_task_store};
use anyhow::{Context, Result, ensure};
use clap::Args;
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
}
#[derive(Debug, Serialize, Deserialize)]
struct ReleaseContext {
    train_id: String,
    owner: String,
    repository: String,
    branch: String,
    integration: String,
    release_commit: Option<String>,
    first_child_commit: Option<String>,
    first_child_epoch: Option<i64>,
    landing_reminder: Option<i64>,
    publish_reminder: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    release_inputs: Option<serde_json::Value>,
}

fn version_ok(version: &str) -> bool {
    let fields: Vec<_> = version.split('.').collect();
    fields.len() == 3
        && fields
            .iter()
            .all(|s| !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
}
fn receipt_path(root: &Path, version: &str) -> Result<PathBuf> {
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
fn git(repo: &Path, args: &[&str]) -> Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()?;
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
    let train = release_train::open_trains(store.as_ref(), &integration)
        .into_iter()
        .next()
        .context("no open release train; create work on the train first")?;
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
        let audit: serde_json::Value = serde_json::from_slice(&fs::read(path)?)?;
        let checked = audit.get("checked_sha").and_then(|value| value.as_str())
            .context("release input audit has no checked_sha")?;
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
    let path = receipt_path(root, version)?;
    if !path.exists() {
        return Ok(false);
    }
    let context: ReleaseContext = serde_json::from_slice(&fs::read(path)?)?;
    if !context.repository.eq_ignore_ascii_case(repo) {
        return Ok(false);
    }
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
    Ok(true)
}

pub fn installed(args: &InstalledArgs, current_root: Option<&Path>) -> Result<()> {
    let publication: serde_json::Value = serde_json::from_slice(&fs::read(&args.receipt)?)?;
    let version = publication["tag_name"]
        .as_str()
        .and_then(|s| s.strip_prefix('v'))
        .context("publication has no version tag")?;
    ensure!(
        version_ok(version) && version == env!("CARGO_PKG_VERSION"),
        "publication must match the installed binary"
    );
    ensure!(
        publication["draft"].as_bool() == Some(false),
        "release is still a draft"
    );
    let published = publication["published_at"]
        .as_str()
        .context("release is not published")?;
    chrono::DateTime::parse_from_rfc3339(published).context("invalid published_at")?;
    let mut roots = std::collections::BTreeSet::new();
    if let Some(root) = current_root {
        roots.insert(root.to_path_buf());
    }
    for repo in crate::worktree::discovery::list_tracked_repos()? {
        if repo.healthy {
            roots.insert(crate::store::CasContext::from_path(&repo.path)?.cas_root);
        }
    }
    let mut delivered = 0;
    for root in roots {
        delivered += usize::from(installed_at_root(&root, &args.repo, version)?);
    }
    println!("Post-release notice recorded for v{version} ({delivered} project(s))");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_types::{Agent, AgentRole, Task, TaskType, WorkTarget};

    fn fixture() -> (tempfile::TempDir, PathBuf, Task, Agent, TrackArgs) {
        let temp = tempfile::TempDir::new().unwrap();
        let repo = temp.path();
        git(repo, &["init", "-b", "main"]).unwrap();
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
    fn release_tracking_and_installed_notice_are_restart_safe() {
        let (temp, root, mut train, owner, mut args) = fixture();
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
        assert_eq!(recorded.release_inputs, Some(audit));
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
        // Core may roll to the next train before the host installs.
        train.status = cas_types::TaskStatus::Closed;
        train.epic_verification_owner = None;
        open_task_store(&root).unwrap().update(&train).unwrap();
        assert!(!installed_at_root(&root, "example/other", &args.version).unwrap());
        for _ in 0..2 {
            assert!(installed_at_root(&root, "example/repo", &args.version).unwrap());
        }
        let rows = crate::store::open_prompt_queue_store(&root)
            .unwrap()
            .peek_all(10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, owner.name);
        assert!(crate::supervisor_notices::is_action_envelope(
            &rows[0].prompt
        ));
    }
    #[test]
    fn release_tracking_owner_fallback_never_blocks() {
        for (recorded_owner, peer_available) in [(false, true), (false, false), (true, false)] {
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
            track_on_train(&args, &root, temp.path(), &train).unwrap();
            let pending = open_reminder_store(&root)
                .unwrap()
                .list_all_pending()
                .unwrap();
            assert_eq!(pending.len(), if peer_available { 2 } else { 0 });
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
}
