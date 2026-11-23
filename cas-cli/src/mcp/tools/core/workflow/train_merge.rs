//! Ordered supervisor merge intents. The daemon publishes one delivery at a
//! time through normal merge admission, in order, while every queued delivery's
//! verifier runs in parallel; a pending verdict retains its position.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use cas_types::{AgentRole, TaskStatus, TaskType, VerificationDispatchState};
use serde::{Deserialize, Serialize};

use crate::mcp::CasCore;
use crate::worktree::{GitOperations, target_lock};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    task: String,
    branch: String,
    sha: String,
}

#[derive(Serialize, Deserialize)]
struct Queue {
    id: String,
    train: String,
    repo: PathBuf,
    target: String,
    supervisor: String,
    #[serde(default)]
    owner_generation: String,
    session: String,
    entries: Vec<Entry>,
    next: usize,
    outcomes: Vec<String>,
    waiting: Option<String>,
    #[serde(default)]
    ci_override: Option<String>,
    #[serde(default)]
    revision: u64,
    #[serde(default)]
    advancement: Option<Advancement>,
    #[serde(default)]
    advancement_reclaims: Vec<AdvancementReclaim>,
}

const ADVANCEMENT_SECONDS: i64 = 120;

#[derive(Clone, Serialize, Deserialize)]
struct Advancement {
    token: String,
    pid: u32,
    #[serde(default)]
    starttime: Option<u64>,
    expires_at: chrono::DateTime<chrono::Utc>,
}

impl Advancement {
    fn stale_reason(&self) -> Option<&'static str> {
        if self.expires_at <= chrono::Utc::now() {
            Some("expired")
        } else if !self.starttime.map_or_else(
            || crate::mcp::daemon::pid_alive(self.pid),
            |stamp| crate::mcp::daemon::pid_matches_fingerprint(self.pid, stamp),
        ) {
            Some("orphaned")
        } else {
            None
        }
    }
}

#[derive(Serialize, Deserialize)]
struct AdvancementReclaim {
    lease: Advancement,
    reason: String,
    reclaimed_at: chrono::DateTime<chrono::Utc>,
}

impl Queue {
    fn active(&self) -> bool {
        self.next < self.entries.len()
    }

    fn reclaim_advancement(&mut self) -> bool {
        if let Some(reason) = self.advancement.as_ref().and_then(Advancement::stale_reason) {
            let lease = self.advancement.take().expect("stale lease");
            tracing::warn!(train = %self.train, token = %lease.token, pid = lease.pid, reason,
                "train advancement lease reclaimed");
            self.advancement_reclaims.push(AdvancementReclaim {
                lease, reason: reason.into(), reclaimed_at: chrono::Utc::now(),
            });
            true
        } else {
            false
        }
    }

    fn render(&self) -> String {
        let mut receipt = format!(
            "Train merge {}: {} of {} processed on {}.{}{}",
            self.id,
            self.next,
            self.entries.len(),
            self.target,
            self.waiting
                .as_ref()
                .map(|s| format!(" Held: {s}"))
                .unwrap_or_default(),
            if self.active() {
                " Singles into this train are held; the daemon resumes this position."
            } else {
                " Queue complete; single merges are released."
            },
        );
        if let Some(reclaimed) = self.advancement_reclaims.last() {
            receipt.push_str(&format!("\nAdvancement lease reclaimed: token={} pid={} reason={} at={}",
                reclaimed.lease.token, reclaimed.lease.pid, reclaimed.reason, reclaimed.reclaimed_at));
        }
        receipt
    }
}

fn directory(root: &Path) -> PathBuf {
    root.join("train-merges")
}

fn path(root: &Path, repo: &Path, target: &str) -> Result<PathBuf> {
    let common = GitOperations::new(repo.to_path_buf()).canonical_repo_key()?;
    Ok(directory(root).join(format!(
        "{}.json",
        target_lock::delivery_target_key(&common, target)
    )))
}

fn read(path: &Path) -> Result<Option<Queue>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

// Only local metadata I/O is inside this lock. Candidate preparation, CI,
// store calls and publication must never retain it.
fn save(path: &Path, queue: &mut Queue) -> Result<()> {
    let root = path.parent().and_then(Path::parent).context("queue root missing")?;
    let common = GitOperations::new(queue.repo.clone()).canonical_repo_key()?;
    let _state = target_lock::lock_delivery_target(root, &common,
        &format!("train-merge-state/{}", queue.target))?;
    let current = read(path)?;
    if let Some(current) = current.as_ref() {
        ensure!(if current.id == queue.id { current.revision == queue.revision }
            else { !current.active() && queue.revision == 0 },
            crate::gates::tag("G-113", "train queue changed during advancement; retry with its current state"));
    } else {
        ensure!(queue.revision == 0, "train queue disappeared during advancement");
    }
    let revision = current.map_or(0, |current| current.revision).checked_add(1)
        .context("train queue revision overflow")?;
    let previous_revision = queue.revision;
    queue.revision = revision;
    let result = write_queue(path, queue);
    if result.is_err() { queue.revision = previous_revision; }
    result
}

fn write_queue(path: &Path, queue: &Queue) -> Result<()> {
    let parent = path.parent().context("queue directory missing")?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(std::io::Write::by_ref(&mut file), queue)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// GH #2316: the queue's recorded supervisor holds it only while that session
/// is live. The caller is already the current train owner, so an ended prior
/// owner's generation is adopted rather than held forever. cas-6456: an
/// unstamped queue continues only while no explicit owner record exists.
fn held_by_prior_owner(root: &Path, actor: &cas_types::Agent, queue: &Queue, epic: &cas_types::Task, generation: &str) -> Result<bool> {
    let continues = crate::cloud::peer_mailbox::owns_recipient(root, actor, &queue.supervisor)
        && crate::train_owner::generation_matches(root, epic, &queue.owner_generation, generation).map_err(anyhow::Error::msg)?;
    Ok(!continues && crate::cloud::peer_mailbox::supervisor_is_live(root, &queue.supervisor))
}

/// Checked before preparing a candidate and again under the normal target
/// lock. The queue creates its reservation under that same lock, so a single
/// merge already in flight cannot slip between reservation and publication.
// gate[G-101 merge-admission integrity]: Train merge active reservation
pub(crate) fn admit(
    core: &CasCore,
    repo: &Path,
    target: &str,
    task: Option<&str>,
    permit: Option<&str>,
) -> Result<()> {
    let root = &core.cas_root;
    let tasks = core.open_task_store().map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let common = GitOperations::new(repo.to_path_buf()).canonical_repo_key()?;
    for epic in tasks.list(None)?.into_iter().filter(|epic|
        epic.task_type == TaskType::Epic && !epic.is_terminal()
            && epic.effective_epic_branch() == Some(target)
            && epic.epic_verification_owner.is_some())
    {
        let declared = match epic.deliverables.work_target.as_ref() {
            Some(work) => super::super::task::repo_context::resolve_repo_context(root, work)
                .map_err(anyhow::Error::msg)?.git_common_dir,
            None => GitOperations::new(root.parent().context("project repository missing")?.to_path_buf())
                .canonical_repo_key()?,
        };
        if declared != common { continue; }
        let actor = core.resolve_live_supervisor_authority()
            .map_err(|error| anyhow::anyhow!(crate::gates::tag("G-113", format!("train owner authority unavailable: {error:?}"))))?;
        crate::factory_session_scope::require_train_owner(root, &epic, &actor).map_err(anyhow::Error::msg)?;
    }
    let queue = read(&path(root, repo, target)?)?;
    if let Some(queue) = queue.filter(Queue::active) {
        let epic = tasks.get(&queue.train)?;
        let actor = core.resolve_live_supervisor_authority()
            .map_err(|error| anyhow::anyhow!(crate::gates::tag("G-113", format!("train owner authority unavailable: {error:?}"))))?;
        let generation = crate::factory_session_scope::require_train_owner(root, &epic, &actor).map_err(anyhow::Error::msg)?;
        ensure!(!held_by_prior_owner(root, &actor, &queue, &epic, &generation)?,
            crate::gates::tag("G-113", "train merge owner generation changed; the prior queue remains held"));
        ensure!(
            permit == Some(queue.id.as_str())
                && task == Some(queue.entries[queue.next].task.as_str()),
            crate::gates::tag("G-101", format!(
                "TRAIN MERGE ACTIVE: {} owns {}; single worktree_merge is held at task {} (position {}/{}). Resume with factory action=train_merge id={}.",
                queue.id,
                target,
                queue.entries[queue.next].task,
                queue.next + 1,
                queue.entries.len(),
                queue.train
            ))
        );
    } else {
        ensure!(
            permit.is_none(),
            crate::gates::tag("G-101", "train merge reservation is no longer active")
        );
    }
    Ok(())
}

/// The train whose active merge owns this target, if any.
pub(crate) fn owner(root: &Path, repo: &Path, target: &str) -> Option<String> {
    read(&path(root, repo, target).ok()?)
        .ok()?
        .filter(Queue::active)
        .map(|queue| queue.train)
}

/// A bounded operation lease, separate from the short queue metadata lock.
/// A reclaimed driver cannot save a stale revision or clear its successor's token.
struct AdvancementGuard {
    path: PathBuf,
    token: String,
}

impl AdvancementGuard {
    fn claim(path: &Path, queue: &mut Queue) -> Result<Option<Self>> {
        queue.reclaim_advancement();
        if queue.advancement.is_some() { return Ok(None); }
        let token = uuid::Uuid::new_v4().to_string();
        let pid = std::process::id();
        queue.advancement = Some(Advancement {
            token: token.clone(), pid, starttime: crate::mcp::daemon::read_pid_starttime(pid),
            expires_at: chrono::Utc::now() + chrono::Duration::seconds(ADVANCEMENT_SECONDS),
        });
        save(path, queue)?;
        Ok(Some(Self { path: path.into(), token }))
    }

    fn current(&self) -> Result<Option<Queue>> {
        let Some(mut queue) = read(&self.path)? else { return Ok(None) };
        let Some(lease) = queue.advancement.as_mut().filter(|lease|
            lease.token == self.token && lease.stale_reason().is_none()) else { return Ok(None) };
        lease.expires_at = chrono::Utc::now() + chrono::Duration::seconds(ADVANCEMENT_SECONDS);
        save(&self.path, &mut queue)?;
        Ok(Some(queue))
    }
}

impl Drop for AdvancementGuard {
    fn drop(&mut self) {
        let release = || -> Result<()> {
            if let Some(mut queue) = read(&self.path)?
                && queue.advancement.as_ref().is_some_and(|lease| lease.token == self.token)
            {
                queue.advancement = None;
                save(&self.path, &mut queue)?;
            }
            Ok(())
        };
        if let Err(error) = release() {
            tracing::debug!(%error, "train advancement release deferred to lease expiry");
        }
    }
}

impl CasCore {
    // gate[G-113 train-merge integrity]: Train merge preconditions (supervisor, epic target, queue, entries)
    // gate-audit[ci_override]: selector Derived newly accepted G-114 token; final receipt records it.
    // gate-audit[prior_override]: selector Replayed stored decision token; reuse does not mint another override.
    pub(crate) fn train_merge(
        &self,
        train: &str,
        ordered: Option<&[String]>,
        supervisor_override: bool,
        reason: Option<&str>,
    ) -> Result<String> {
        let actor = self
            .resolve_live_supervisor_authority()
            .map_err(|_| anyhow::anyhow!(crate::gates::tag("G-113", "train_merge requires a live registered supervisor")))?;
        if let Some(hold) = cas_store::project_hold::notice(&self.cas_root) {
            anyhow::bail!(crate::gates::tag("G-92", hold));
        }
        let session = actor
            .factory_session
            .as_deref()
            .context(crate::gates::tag("G-113", "train_merge requires a factory session"))?;
        let tasks = self
            .open_task_store()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let epic = tasks.get(train)?;
        ensure!(
            epic.task_type == TaskType::Epic && !epic.is_terminal(),
            crate::gates::tag("G-113", "train_merge requires an open epic")
        );
        let generation = crate::factory_session_scope::require_train_owner(&self.cas_root, &epic, &actor)
            .map_err(anyhow::Error::msg)?;
        let target = epic
            .effective_epic_branch()
            .context(crate::gates::tag("G-113", "train has no branch"))?
            .to_string();
        ensure!(
            target.starts_with("epic/"),
            crate::gates::tag("G-113", "train_merge requires an epic target, never trunk")
        );
        let repo = match epic.deliverables.work_target.as_ref() {
            Some(work) => {
                super::super::task::repo_context::resolve_repo_context(&self.cas_root, work)
                    .map_err(anyhow::Error::msg)?
                    .repo_root
            }
            None => self
                .cas_root
                .parent()
                .context("project repository missing")?
                .to_path_buf(),
        };
        let path = path(&self.cas_root, &repo, &target)?;
        if let Some(mut queue) = read(&path)?.filter(Queue::active) {
            ensure!(queue.train == train, crate::gates::tag("G-113", "another train owns this target"));
            ensure!(!held_by_prior_owner(&self.cas_root, &actor, &queue, &epic, &generation)?,
                crate::gates::tag("G-113", "train merge owner generation changed; the prior queue remains held"));
            if queue.reclaim_advancement() { save(&path, &mut queue)?; }
            queue.supervisor = actor.id.clone();
            queue.owner_generation = generation.clone();
            // GH #1819: entries already merged or closed need no daemon pass.
            settle(&self.cas_root, &path, &mut queue, &*tasks)?;
            let prior_override = queue.ci_override.clone();
            if let Some(ordered) = ordered {
                let existing = queue
                    .entries
                    .iter()
                    .map(|entry| entry.task.clone())
                    .collect::<std::collections::BTreeSet<_>>();
                let append = ordered
                    .iter()
                    .filter(|id| !existing.contains(*id))
                    .cloned()
                    .collect::<Vec<_>>();
                if !append.is_empty() {
                    let agents = self
                        .open_agent_store()
                        .map_err(|e| anyhow::anyhow!(e.to_string()))?
                        .list(None)?;
                    let mut seen = existing;
                    let additions = validated_entries(
                        &self.cas_root,
                        &repo,
                        &target,
                        train,
                        session,
                        &*tasks,
                        &agents,
                        append,
                        &mut seen,
                    )?;
                    queue.entries.extend(additions);
                }
            }
            if queue.active() {
                queue.ci_override = crate::train_ci::guard(
                    self,
                    train,
                    &target,
                    queue.ci_override.as_deref(),
                    supervisor_override,
                    reason,
                )?;
                queue.session = session.into();
            }
            save(&path, &mut queue)?;
            // Acknowledge the durable revision before candidate preparation,
            // origin reads or verifier handoffs. The daemon's resume pass
            // starts appended reviews and retries a pending head (cas-7f934).
            let positions = queue.entries.iter().enumerate()
                .filter(|(_, entry)| ordered.is_some_and(|ids| ids.contains(&entry.task)))
                .map(|(index, entry)| format!("{}={}", entry.task, index + 1))
                .collect::<Vec<_>>().join(", ");
            let receipt = format!("{} Committed revision {}. Queued positions: [{}].{}",
                queue.render(), queue.revision, positions,
                crate::ops::fleet::auto_focus_epic(&self.cas_root, session, train));
            return Ok(if queue.ci_override.is_some() && queue.ci_override != prior_override {
                crate::gates::override_tag("G-114", receipt)
            } else { receipt });
        }
        let ci_override =
            crate::train_ci::guard(self, train, &target, None, supervisor_override, reason)?;
        let mut candidates = tasks.list(Some(TaskStatus::AwaitingMerge))?;
        candidates.sort_by(|a, b| (a.updated_at, &a.id).cmp(&(b.updated_at, &b.id)));
        let ids = match ordered {
            Some(ids) => ids.to_vec(),
            None => {
                let mut ids = Vec::new();
                for task in candidates {
                    if tasks
                        .get_parent_epic(&task.id)?
                        .is_some_and(|parent| parent.id == train)
                    {
                        ids.push(task.id);
                    }
                }
                ids
            }
        };
        let agents = self
            .open_agent_store()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?
            .list(None)?;
        let mut seen = std::collections::BTreeSet::new();
        let entries = validated_entries(
            &self.cas_root,
            &repo,
            &target,
            train,
            session,
            &*tasks,
            &agents,
            ids,
            &mut seen,
        )?;
        let mut queue = Queue {
            id: uuid::Uuid::new_v4().to_string(),
            train: train.into(),
            repo: repo.clone(),
            target: target.clone(),
            supervisor: actor.id,
            owner_generation: generation,
            session: session.into(),
            entries,
            next: 0,
            outcomes: vec![],
            waiting: None,
            ci_override,
            revision: 0,
            advancement: None,
            advancement_reclaims: vec![],
        };
        let common = GitOperations::new(repo).canonical_repo_key()?;
        let target_lock = target_lock::try_lock_delivery_target(&self.cas_root, &common, &target)?
            .context(crate::gates::tag("G-113", "target merge already in progress; retry train_merge after it finishes"))?;
        save(&path, &mut queue)?;
        // GH #2169: every queued delivery's review starts now, outside the publication lock.
        drop(target_lock);
        start_queued_reviews(self, &queue, &*tasks, 0);
        let receipt = format!("{}{}", queue.render(), crate::ops::fleet::auto_focus_epic(&self.cas_root, session, train));
        Ok(if queue.ci_override.is_some() { crate::gates::override_tag("G-114", receipt) } else { receipt })
    }
}

fn validated_entries(
    root: &Path,
    repo: &Path,
    target: &str,
    train: &str,
    session: &str,
    tasks: &dyn crate::store::TaskStore,
    agents: &[cas_types::Agent],
    ids: Vec<String>,
    seen: &mut std::collections::BTreeSet<String>,
) -> Result<Vec<Entry>> {
    let common = GitOperations::new(repo.to_path_buf()).canonical_repo_key()?;
    let mut entries = Vec::with_capacity(ids.len());
    for id in ids {
        ensure!(
            seen.insert(id.clone()),
            crate::gates::tag("G-113", format!("duplicate train delivery {id}"))
        );
        let task = tasks.get(&id)?;
        ensure!(
            task.status == TaskStatus::AwaitingMerge,
            crate::gates::tag("G-113", format!("{id} is not parked"))
        );
        ensure!(
            tasks
                .get_parent_epic(&id)?
                .is_some_and(|parent| parent.id == train),
            crate::gates::tag("G-113", format!("{id} belongs to another train"))
        );
        if let Some(owner) = crate::factory_session_scope::foreign_owner_of_merge(
            agents,
            Some(session),
            task.deliverables.parked_branch.as_deref().unwrap_or(""),
            Some(&task),
            chrono::Utc::now(),
        ) {
            anyhow::bail!(crate::gates::tag("G-93", owner.refusal("train_merge", session)));
        }
        if let Some(work) = task.deliverables.work_target.as_ref() {
            let context = super::super::task::repo_context::resolve_repo_context(root, work)
                .map_err(anyhow::Error::msg)?;
            ensure!(
                context.target_branch == target && context.git_common_dir == common,
                crate::gates::tag("G-113", format!("{id} targets another repository or branch"))
            );
        }
        entries.push(Entry {
            task: id,
            branch: task
                .deliverables
                .parked_branch
                .context(crate::gates::tag("G-113", "parked delivery lacks branch"))?,
            sha: task
                .deliverables
                .factory_branch_anchor
                .context(crate::gates::tag("G-113", "parked delivery lacks anchor"))?,
        });
    }
    Ok(entries)
}

/// Advance past entries that need no merge: already published to the target,
/// or no longer this exact parked delivery. Each step is durable, so a resume
/// after a crash repeats nothing.
fn settle(
    root: &Path,
    path: &Path,
    queue: &mut Queue,
    tasks: &dyn crate::store::TaskStore,
) -> Result<()> {
    while queue.active() {
        let entry = &queue.entries[queue.next];
        // gate[G-115 train-merge integrity]: Advance only published, rejected or changed entries
        let outcome = if published(queue, entry, false)? {
            format!("{} merged {}", entry.task, entry.sha)
        } else {
            let task = tasks.get(&entry.task)?;
            if task.status == TaskStatus::AwaitingMerge
                && task.deliverables.factory_branch_anchor.as_deref() == Some(&entry.sha)
                && task.deliverables.parked_branch.as_deref() == Some(&entry.branch)
            {
                let rejected = match cas_store::get_latest_verification_dispatch(root, &entry.task)? {
                    Some(dispatch) if dispatch.state == VerificationDispatchState::Resolved => {
                        cas_store::get_verification_for_dispatch(root, &dispatch.id)?
                            .is_some_and(|verdict| {
                                verdict.status == cas_types::VerificationStatus::Rejected
                            })
                    }
                    _ => false,
                };
                if rejected {
                    format!("{} rejected verification", entry.task)
                } else {
                    return Ok(());
                }
            } else {
                crate::gates::tag("G-115", format!(
                    "{} returned or changed; skipped exact parked delivery",
                    entry.task
                ))
            }
        };
        queue.outcomes.push(outcome);
        queue.next += 1;
        queue.waiting = None;
        save(path, queue)?;
    }
    Ok(())
}

/// Publication, rather than a local-only merge, advances the queue. The merge's
/// origin read and push refresh the tracking ref; ask origin only if it lacks one.
fn published(queue: &Queue, entry: &Entry, merged: bool) -> Result<bool> {
    let git = GitOperations::new(queue.repo.clone());
    let tracked = git.resolve_commit(&format!("refs/remotes/origin/{}", queue.target))
        .is_some_and(|tip| git.is_ancestor(&entry.sha, &tip));
    if tracked || (!merged && git.has_origin_remote()) { return Ok(tracked); }
    let target = git
        .fetch_merge_target(&queue.target)?
        .or_else(|| git.resolve_commit(&queue.target))
        .context("train target missing")?;
    Ok(git.is_ancestor(&entry.sha, &target))
}

/// GH #2169: only publication is ordered. Every delivery from `from` on gets
/// its candidate and verifier through the same preparation the head uses.
/// Enqueue starts reviews immediately; active appends leave them to the daemon,
/// so their reviews run while earlier entries publish. A publish that changes a
/// later entry's reviewed files is re-reviewed by that preparation (G-104) when
/// the entry reaches the head. A hold on its own dispatch is the expected
/// answer here; any other refusal surfaces again when the entry is the head.
fn start_queued_reviews(core: &CasCore, queue: &Queue, tasks: &dyn crate::store::TaskStore, from: usize) {
    for entry in queue.entries.iter().skip(from) {
        let Ok(task) = tasks.get(&entry.task) else { continue };
        if task.status != TaskStatus::AwaitingMerge
            || task.deliverables.factory_branch_anchor.as_deref() != Some(&entry.sha)
            || task.deliverables.parked_branch.as_deref() != Some(&entry.branch)
            || published(queue, entry, false).unwrap_or(false)
        {
            continue;
        }
        if let Err(error) = core.prepare_premerge_verification(&task, &queue.repo, &entry.branch, &queue.target) {
            tracing::debug!(task = %entry.task, reason = %error.message, "queued train delivery review not started");
        }
    }
}

/// Existing daemon scan, off its control loop. No additional CI stage or
/// model turn is needed to consume an approved candidate.
// gate-audit[ci_override]: selector Derived accepted G-114 token forwarded in response; not another request flag.
pub(crate) async fn resume_pending(root: &Path, session: &str) -> Result<()> {
    let unchecked = crate::mcp::tools::core::task::lifecycle::parked_conflicts::scan(root)?;
    let paths = match std::fs::read_dir(directory(root)) {
        Ok(paths) => paths,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e.into()),
    };
    for entry in paths {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) != Some("json") {
            continue;
        }
        let Some(queue) = read(&path)? else {
            continue;
        };
        if !queue.active() || queue.session != session {
            continue;
        }
        let Some(mut queue) = read(&path)? else { continue };
        let Some(driver) = AdvancementGuard::claim(&path, &mut queue)? else { continue };
        // GH #2017: only a queue whose own target missed its conflict scan waits.
        if let Some(reason) = unchecked.get(&queue.target) {
            if queue.waiting.as_ref() != Some(reason) {
                queue.waiting = Some(reason.clone());
                save(&path, &mut queue)?;
            }
            continue;
        }
        // GH #2018: without a daemon, an entry's verifier dispatch got only a
        // manual worktree_merge handoff, which G-101 refuses during the train.
        let daemon = crate::mcp::daemon::EmbeddedDaemon::new(crate::mcp::daemon::EmbeddedDaemonConfig {
            cas_root: root.to_path_buf(),
            cloud_sync_enabled: false,
            ..Default::default()
        });
        let core = CasCore::with_daemon(root.to_path_buf(), None, Some(std::sync::Arc::new(daemon)));
        core.bind_agent_identity(
            queue.supervisor.clone(),
            crate::mcp::AgentIdentitySource::ServerInternal,
        )
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let actor = core.resolve_live_supervisor_authority().map_err(|_| {
            anyhow::anyhow!(crate::gates::tag("G-113", "train merge owner is no longer live; resume with train_merge"))
        })?;
        ensure!(
            actor.role == AgentRole::Supervisor
                && actor.factory_session.as_deref() == Some(session),
            crate::gates::tag("G-113", "train merge owner session changed")
        );
        let tasks = core
            .open_task_store()
            .map_err(|e| anyhow::anyhow!(e.to_string()))?;
        let epic = tasks.get(&queue.train)?;
        let generation = crate::factory_session_scope::require_train_owner(root, &epic, &actor)
            .map_err(anyhow::Error::msg)?;
        ensure!(crate::train_owner::generation_matches(root, &epic, &queue.owner_generation, &generation).map_err(anyhow::Error::msg)?,
            crate::gates::tag("G-113", "train merge owner generation changed"));
        loop {
            let Some(current) = driver.current()? else { break };
            queue = current;
            // Settled entries advance even while CI holds the next merge.
            settle(root, &path, &mut queue, &*tasks)?;
            if !queue.active() {
                break;
            }
            start_queued_reviews(&core, &queue, &*tasks, queue.next + 1);
            let Some(current) = driver.current()? else { break };
            queue = current;
            if !queue.active() { break; }
            if let Err(error) = crate::train_ci::guard(
                &core,
                &queue.train,
                &queue.target,
                queue.ci_override.as_deref(),
                false,
                None,
            ) {
                queue.waiting = Some(error.to_string());
                save(&path, &mut queue)?;
                break;
            }
            let entry = queue.entries[queue.next].clone();
            if let Some(dispatch) = cas_store::get_latest_verification_dispatch(root, &entry.task)?
                && matches!(
                    dispatch.state,
                    VerificationDispatchState::Pending | VerificationDispatchState::Claimed
                )
            {
                // GH #2018: an unclaimed dispatch needs its daemon verifier
                // requested here; nothing else starts it while the train holds.
                if dispatch.state == VerificationDispatchState::Pending {
                    core.emit_premerge_review_handoff(&tasks.get(&entry.task)?, &dispatch)
                        .map_err(|error| anyhow::anyhow!(error.message.to_string()))?;
                }
                queue.waiting = Some(crate::gates::tag("G-115", format!("{} awaiting dispatch {}", entry.task, dispatch.id)));
                save(&path, &mut queue)?;
                break;
            }
            let Some(current_queue) = driver.current()? else { break };
            queue = current_queue;
            if !queue.active() || queue.entries[queue.next].task != entry.task { continue; }
            let expected = crate::factory_ci_retry::Retry {
                task: entry.task.clone(),
                worktree: entry.branch.clone(),
                branch: entry.branch.clone(),
                sha: entry.sha.clone(),
                ci_sha: entry.sha.clone(),
                target: queue.target.clone(),
                repo: queue.repo.clone(),
                supervisor: queue.supervisor.clone(),
                session: queue.session.clone(),
                force: false,
                allow_trunk: false,
                cleanup: Some(false),
                armed_at: None,
                ci_rerun: Vec::new(),
            };
            let result = core.worktree_merge_queued(&expected, &queue.id).await;
            let Some(current_queue) = driver.current()? else { break };
            queue = current_queue;
            if !queue.active() || queue.entries[queue.next].task != entry.task { continue; }
            let current = tasks.get(&entry.task)?;
            if current.status != TaskStatus::AwaitingMerge || published(&queue, &entry, true)? {
                // Re-read the durable result at the top, including a conflict
                // return or a merge committed just before a previous crash.
                continue;
            }
            queue.waiting = Some(match result {
                Err(error) => error.message.to_string(),
                Ok(result) => result
                    .content
                    .iter()
                    .filter_map(|c| match c {
                        rmcp::model::ContentBlock::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            });
            save(&path, &mut queue)?;
            break;
        }
        if !queue.active() {
            let agents = core
                .open_agent_store()
                .map_err(|e| anyhow::anyhow!(e.to_string()))?
                .list(None)?;
            let recipient =
                crate::supervisor_notices::resolve(&agents, Some(&queue.supervisor), None);
            crate::supervisor_notices::enqueue(
                root,
                &recipient,
                "train_merge",
                &format!("{}\n{}", queue.render(), queue.outcomes.join("\n")),
                &queue.id,
            )?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_types::{Agent, Task, TaskDepth};
    use rmcp::handler::server::wrapper::Parameters;

    fn git(repo: &Path, args: &[&str]) -> String {
        let output = std::process::Command::new("git")
            .current_dir(repo)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().into()
    }

    fn commit(repo: &Path, file: &str, body: &str) -> String {
        std::fs::write(repo.join(file), body).unwrap();
        git(repo, &["add", file]);
        git(repo, &["commit", "-qm", file]);
        git(repo, &["rev-parse", "HEAD"])
    }

    struct Fixture {
        _repo: tempfile::TempDir,
        _env: crate::test_support::TestEnvGuard,
        root: PathBuf,
        core: CasCore,
        ids: Vec<String>,
        shas: Vec<String>,
    }

    impl Fixture {
        fn new() -> Self {
            Self::build(true)
        }

        /// `conflict`: the first two deliveries edit the same file.
        fn build(conflict: bool) -> Self {
            let mut env = crate::test_support::TestEnvGuard::temp_home();
            for key in [
                "CAS_ROOT",
                "CAS_CLONE_PATH",
                "CAS_AGENT_ROLE",
                "CAS_AGENT_NAME",
                "CAS_FACTORY_SESSION",
                "CAS_SESSION_ID",
            ] {
                env.remove(key);
            }
            let repo = tempfile::tempdir().unwrap();
            git(repo.path(), &["init", "-qb", "main"]);
            git(repo.path(), &["config", "user.name", "Cassy test"]);
            git(
                repo.path(),
                &["config", "user.email", "test@example.invalid"],
            );
            commit(repo.path(), ".gitignore", ".cas/\n");
            commit(repo.path(), "shared.md", "base\n");
            git(repo.path(), &["branch", "epic/train"]);
            let root = crate::store::init_cas_dir(repo.path()).unwrap();
            std::fs::write(
                root.join("config.toml"),
                "[verification]\nenabled=false\n[qa]\nmode='batch'\n",
            )
            .unwrap();
            let origin = root.join("origin.git");
            git(repo.path(), &["init", "--bare", origin.to_str().unwrap()]);
            git(
                repo.path(),
                &["remote", "add", "origin", origin.to_str().unwrap()],
            );
            git(repo.path(), &["push", "origin", "main", "epic/train"]);
            let core = CasCore::with_daemon(root.clone(), None, None);
            core.set_agent_id_for_testing("supervisor".into());
            let agents = core.open_agent_store().unwrap();
            let mut actor = Agent::new("supervisor".into(), "supervisor".into());
            actor.role = AgentRole::Supervisor;
            actor.factory_session = Some("queue-session".into());
            agents.register(&actor).unwrap();
            let tasks = core.open_task_store().unwrap();
            let mut epic = Task::new("cas-train".into(), "train".into());
            epic.task_type = TaskType::Epic;
            epic.branch = Some("epic/train".into());
            epic.epic_verification_owner = Some(actor.id.clone());
            tasks.add(&epic).unwrap();
            let ids: Vec<String> = (1..=3).map(|i| format!("cas-delivery-{i}")).collect();
            let mut shas = vec![String::new(); 3];
            // Create in reverse order: the explicit merge order wins over
            // store insertion order and timestamps.
            for index in [2, 1, 0] {
                let worker = format!("worker-{}", index + 1);
                let branch = format!("factory/{worker}");
                let checkout = root.join("worktrees").join(&worker);
                git(
                    repo.path(),
                    &[
                        "worktree",
                        "add",
                        "-qb",
                        &branch,
                        checkout.to_str().unwrap(),
                        "epic/train",
                    ],
                );
                let file = match index {
                    0 => "shared.md",
                    1 if conflict => "shared.md",
                    1 => "second.md",
                    _ => "third.md",
                };
                shas[index] = commit(&checkout, file, &format!("delivery {}\n", index + 1));
                git(&checkout, &["push", "origin", &branch]);
                let mut agent = Agent::new(worker.clone(), worker.clone());
                agent.role = AgentRole::Worker;
                agent.factory_session = Some("queue-session".into());
                agent
                    .metadata
                    .insert("clone_path".into(), checkout.to_string_lossy().into_owned());
                agents.register(&agent).unwrap();
                let mut task = Task::new(ids[index].clone(), worker.clone());
                task.depth = TaskDepth::Light;
                task.status = TaskStatus::AwaitingMerge;
                task.assignee = Some(worker);
                task.deliverables.factory_branch_anchor = Some(shas[index].clone());
                task.deliverables.parked_branch = Some(branch);
                tasks
                    .create_atomic(&task, &[], Some(&epic.id), None)
                    .unwrap();
            }
            Self {
                _repo: repo,
                _env: env,
                root,
                core,
                ids,
                shas,
            }
        }

        async fn start(&self) {
            self.start_with(&self.ids).await;
        }

        async fn start_with(&self, ids: &[String]) {
            let service = crate::mcp::CasService::new(self.core.clone(), None);
            let request = serde_json::from_value(serde_json::json!({
                "action": "train_merge", "id": "cas-train", "task_ids": ids
            }))
            .unwrap();
            let result = service.factory(Parameters(request)).await.unwrap();
            assert_ne!(result.is_error, Some(true), "{result:?}");
        }
    }

    /// A dead process releases no train authority. Only the pinned owner
    /// may admit this train, even when another supervisor can run locally.
    #[tokio::test]
    async fn train_owner_remains_required_after_previous_process_exits_cas_b674() {
        let fixture = Fixture::new();
        fixture.start().await;
        let agents = fixture.core.open_agent_store().unwrap();
        let mut successor = Agent::new("supervisor-2".into(), "supervisor-2".into());
        successor.role = AgentRole::Supervisor;
        successor.factory_session = Some("queue-session".into());
        agents.register(&successor).unwrap();
        let takeover = || async {
            let core = CasCore::with_daemon(fixture.root.clone(), None, None);
            core.set_agent_id_for_testing("supervisor-2".into());
            let service = crate::mcp::CasService::new(core, None);
            let request = serde_json::from_value(serde_json::json!({
                "action": "train_merge", "id": "cas-train", "task_ids": fixture.ids
            }))
            .unwrap();
            service.factory(Parameters(request)).await
        };

        // The owner's recorded process is this test: live, so it keeps the train.
        let mut owner = agents.get("supervisor").unwrap();
        owner.pid = Some(std::process::id());
        agents.update(&owner).unwrap();
        let held = takeover().await;
        assert!(held.is_err() || held.unwrap().is_error == Some(true));

        // Its process exits; the row stays `active`.
        let mut child = std::process::Command::new("true").spawn().unwrap();
        owner.pid = Some(child.id());
        child.wait().unwrap();
        agents.update(&owner).unwrap();
        let held = takeover().await;
        assert!(held.is_err() || held.unwrap().is_error == Some(true));
        let queue = read(&path(&fixture.root, fixture._repo.path(), "epic/train").unwrap()).unwrap().unwrap();
        assert_eq!(queue.supervisor, "supervisor");
    }

    /// GH #2316: a train handed to a new live owner is held only by a live
    /// prior owner. Once that session ends, the current owner adopts the queue.
    #[tokio::test]
    async fn ended_prior_owner_queue_is_adopted_by_current_train_owner_gh_2316() {
        let fixture = Fixture::new();
        fixture.start().await;
        let agents = fixture.core.open_agent_store().unwrap();
        let mut successor = Agent::new("supervisor-2".into(), "supervisor-2".into());
        successor.role = AgentRole::Supervisor;
        successor.factory_session = Some("queue-session".into());
        agents.register(&successor).unwrap();
        let tasks = fixture.core.open_task_store().unwrap();
        let mut epic = tasks.get("cas-train").unwrap();
        epic.epic_verification_owner = Some("supervisor-2".into());
        tasks.update(&epic).unwrap();
        let takeover = || {
            let core = CasCore::with_daemon(fixture.root.clone(), None, None);
            core.set_agent_id_for_testing("supervisor-2".into());
            core.train_merge("cas-train", Some(&fixture.ids), false, None)
        };
        let queue = || read(&path(&fixture.root, fixture._repo.path(), "epic/train").unwrap()).unwrap().unwrap();

        let held = takeover().unwrap_err().to_string();
        assert!(held.contains("[G-113]"), "{held}");
        assert_eq!(queue().supervisor, "supervisor");

        let mut prior = agents.get("supervisor").unwrap();
        prior.status = cas_types::AgentStatus::Shutdown;
        agents.update(&prior).unwrap();
        takeover().unwrap();
        let adopted = queue();
        assert_eq!(adopted.supervisor, "supervisor-2");
        assert_eq!(adopted.entries.len(), fixture.ids.len());
    }

    #[tokio::test]
    async fn train_merge_requires_registered_supervisor() {
        let _env = crate::test_support::TestEnvGuard::temp_home();
        let temp = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(temp.path()).unwrap();
        let core = CasCore::with_daemon(root, None, None);
        core.set_agent_id_for_testing("worker".into());
        let mut worker = Agent::new("worker".into(), "worker".into());
        worker.role = AgentRole::Worker;
        core.open_agent_store().unwrap().register(&worker).unwrap();
        let service = crate::mcp::CasService::new(core, None);
        let request = serde_json::from_value(serde_json::json!({
            "action": "train_merge", "id": "cas-train"
        }))
        .unwrap();
        let error = service
            .factory(Parameters(request))
            .await
            .expect_err("worker cannot reserve a train");
    }

    #[tokio::test]
    async fn single_candidate_conflict_returns_delivery_to_worker_cas_225b() {
        let mut fixture = Fixture::new();
        fixture._env.set("CAS_FACTORY_WORKER_CLI", "claude");
        fixture._env.set("CAS_FACTORY_SUPERVISOR_CLI", "claude");
        std::fs::write(
            fixture.root.join("config.toml"),
            "[verification]\nenabled=true\n[qa]\nmode='batch'\n",
        ).unwrap();
        let tasks = fixture.core.open_task_store().unwrap();
        let mut task = tasks.get(&fixture.ids[1]).unwrap();
        task.depth = TaskDepth::Deep;
        tasks.update(&task).unwrap();
        // A recorded approval requires candidate preparation even when the
        // release would otherwise own review. No train queue is reserved.
        let dispatch = cas_store::create_verification_dispatch(
            &fixture.root, &task.id, "supervisor", "supervisor",
            chrono::Utc::now() + chrono::Duration::minutes(10),
        ).unwrap();
        let mut verdict = cas_types::Verification::new("ver-approved".into(), task.id.clone());
        verdict.status = cas_types::VerificationStatus::Approved;
        verdict.provenance = cas_types::VerificationProvenance::System;
        verdict.dispatch_id = Some(dispatch.id);
        cas_store::add_system_verification(&fixture.root, &verdict).unwrap();
        git(fixture._repo.path(), &["switch", "-q", "epic/train"]);
        let target = commit(fixture._repo.path(), "shared.md", "target changed after review\n");
        git(fixture._repo.path(), &["push", "origin", "epic/train"]);

        let error = fixture.core.worktree_merge(
            "factory/worker-2", false, Some(&task.id), false, None, false, None,
        ).await.unwrap_err();

        let returned = tasks.get(&task.id).unwrap();
        assert_eq!(returned.status, TaskStatus::Open, "{error}");
        assert_eq!(error.code, rmcp::model::ErrorCode::INTERNAL_ERROR);
        assert_eq!(returned.assignee, task.assignee);
        assert!(returned.deliverables.factory_branch_anchor.is_none());
        assert_eq!(cas_store::get_latest_verification_dispatch(&fixture.root, &task.id)
            .unwrap().unwrap().state, VerificationDispatchState::Invalidated);
        assert!(returned.notes.contains("shared.md"), "{}", returned.notes);
        assert!(error.message.contains("shared.md"), "{error}");
        assert_eq!(GitOperations::new(fixture._repo.path().to_path_buf())
            .resolve_commit("epic/train").as_deref(), Some(target.as_str()));
        assert!(git(fixture._repo.path(), &["status", "--porcelain"]).is_empty());
        assert!(!fixture._repo.path().join(".git/MERGE_HEAD").exists());
    }

    #[tokio::test]
    async fn resume_finishes_queue_whose_deliveries_already_landed_gh_1819() {
        let fixture = Fixture::new();
        fixture.start().await;
        let ci_dir = crate::train_ci::directory(&fixture.root, "cas-train").unwrap();
        std::fs::create_dir_all(&ci_dir).unwrap();
        let red = serde_json::json!({"branch":"epic/train", "head":"bad-tip", "red_head":"bad-tip",
            "red_since":"bad-tip", "run_id":"17", "run_attempt":1, "outcome":"FAIL"});
        std::fs::write(ci_dir.join("state.json"), red.to_string()).unwrap();
        // The first delivery landed through a single merge; the rest closed.
        let refspec = format!("{}:refs/heads/epic/train", fixture.shas[0]);
        git(fixture._repo.path(), &["push", "-q", "origin", &refspec]);
        let tasks = fixture.core.open_task_store().unwrap();
        for id in &fixture.ids[1..] {
            let mut task = tasks.get(id).unwrap();
            task.status = TaskStatus::Closed;
            tasks.update(&task).unwrap();
        }
        let service = crate::mcp::CasService::new(fixture.core.clone(), None);
        let request = serde_json::from_value(serde_json::json!({
            "action": "train_merge", "id": "cas-train"
        }))
        .unwrap();
        let result = service.factory(Parameters(request)).await.unwrap();
        assert_ne!(result.is_error, Some(true), "{result:?}");
        let queue = read(&path(&fixture.root, fixture._repo.path(), "epic/train").unwrap())
            .unwrap()
            .unwrap();
        assert!(!queue.active(), "{}", queue.render());
        assert_eq!(queue.outcomes.len(), 3, "{:?}", queue.outcomes);
        admit(&fixture.core, fixture._repo.path(), "epic/train", Some("cas-later"), None)
            .unwrap();
    }

    #[tokio::test]
    async fn rejected_delivery_drops_its_position_and_active_train_accepts_appends() {
        let mut fixture = Fixture::build(false);
        fixture._env.set("CAS_FACTORY_WORKER_CLI", "claude");
        fixture._env.set("CAS_FACTORY_SUPERVISOR_CLI", "claude");
        std::fs::write(
            fixture.root.join("config.toml"),
            "[verification]\nenabled=true\n[qa]\nmode='batch'\n",
        )
        .unwrap();
        let tasks = fixture.core.open_task_store().unwrap();
        let mut rejected_task = tasks.get(&fixture.ids[0]).unwrap();
        rejected_task.depth = TaskDepth::Deep;
        tasks.update(&rejected_task).unwrap();

        fixture.start_with(&fixture.ids[..1]).await;
        // Let the real queued merge path prepare its exact pre-merge dispatch.
        resume_pending(&fixture.root, "queue-session").await.unwrap();
        let dispatch = cas_store::get_latest_verification_dispatch(&fixture.root, &fixture.ids[0])
            .unwrap()
            .expect("queued merge prepares a pre-merge verification dispatch");
        let service = crate::mcp::CasService::new(fixture.core.clone(), None);
        let rejected = service
            .verification(Parameters(
                serde_json::from_value(serde_json::json!({
                    "action":"add", "task_id":fixture.ids[0], "dispatch_id":dispatch.id,
                    "status":"rejected", "summary":"Reject this exact train candidate",
                }))
                .unwrap(),
            ))
            .await
            .unwrap();
        assert_ne!(rejected.is_error, Some(true), "{rejected:?}");
        assert_eq!(
            cas_store::get_verification_for_dispatch(&fixture.root, &dispatch.id)
                .unwrap()
                .unwrap()
                .status,
            cas_types::VerificationStatus::Rejected
        );

        // Reproduce the incident: another task list arrives while this run is
        // active. It appends in order, and the rejected position is settled.
        let appended = service
            .factory(Parameters(
                serde_json::from_value(serde_json::json!({
                    "action":"train_merge", "id":"cas-train",
                    "task_ids":fixture.ids[1..],
                }))
                .unwrap(),
            ))
            .await
            .unwrap();
        assert_ne!(appended.is_error, Some(true), "{appended:?}");
        let queue_path = path(&fixture.root, fixture._repo.path(), "epic/train").unwrap();
        let queued = read(&queue_path).unwrap().unwrap();
        assert_eq!(queued.next, 1, "{}", queued.render());
        assert_eq!(queued.entries.len(), 3, "{}", queued.render());
        assert!(queued.outcomes[0].contains(&fixture.ids[0]), "{:?}", queued.outcomes);
        let still_parked = tasks.get(&fixture.ids[0]).unwrap();
        assert_eq!(still_parked.status, TaskStatus::AwaitingMerge);
        assert_eq!(
            still_parked.deliverables.factory_branch_anchor.as_deref(),
            Some(fixture.shas[0].as_str())
        );

        resume_pending(&fixture.root, "queue-session").await.unwrap();
        let complete = read(&queue_path).unwrap().unwrap();
        assert!(!complete.active(), "{}", complete.render());
        let operations = GitOperations::new(fixture._repo.path().to_path_buf());
        assert!(!operations.is_ancestor(&fixture.shas[0], "origin/epic/train"));
        assert!(operations.is_ancestor(&fixture.shas[1], "origin/epic/train"));
        assert!(operations.is_ancestor(&fixture.shas[2], "origin/epic/train"));
        assert_eq!(tasks.get(&fixture.ids[0]).unwrap().status, TaskStatus::AwaitingMerge);
    }

    /// The MCP acknowledgement must not wait for origin while starting a
    /// queued review. Hold the real upload-pack until the append has returned.
    #[cfg(unix)]
    #[tokio::test]
    async fn active_append_returns_before_slow_advancement_cas_7f934() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::time::{Duration, Instant};

        let mut fixture = Fixture::build(false);
        fixture._env.set("CAS_FACTORY_WORKER_CLI", "claude");
        fixture._env.set("CAS_FACTORY_SUPERVISOR_CLI", "claude");
        std::fs::write(fixture.root.join("config.toml"),
            "[verification]\nenabled=true\n[qa]\nmode='batch'\n").unwrap();
        let tasks = fixture.core.open_task_store().unwrap();
        let mut appended = tasks.get(&fixture.ids[1]).unwrap();
        appended.depth = TaskDepth::Deep;
        tasks.update(&appended).unwrap();
        fixture.start_with(&fixture.ids[..1]).await;
        let queue_path = path(&fixture.root, fixture._repo.path(), "epic/train").unwrap();
        let before = read(&queue_path).unwrap().unwrap();

        let release = fixture.root.join("release-upload-pack");
        assert!(std::process::Command::new("mkfifo").arg(&release).status().unwrap().success());
        // O_RDWR opens without waiting for the child and retains a release
        // token even when the fixed path has not yet contacted origin.
        let mut release = std::fs::OpenOptions::new().read(true).write(true).open(&release).unwrap();
        let started = fixture.root.join("upload-pack-started");
        let gate = fixture.root.join("gated-upload-pack");
        std::fs::write(&gate, format!(
            "#!/bin/sh\nif mkdir '{}' 2>/dev/null; then read -r release < '{}'; fi\nexec git upload-pack \"$@\"\n",
            started.display(), fixture.root.join("release-upload-pack").display(),
        )).unwrap();
        std::fs::set_permissions(&gate, std::fs::Permissions::from_mode(0o755)).unwrap();
        git(fixture._repo.path(), &["config", "remote.origin.uploadpack", gate.to_str().unwrap()]);

        let service = crate::mcp::CasService::new(fixture.core.clone(), None);
        let request = serde_json::from_value(serde_json::json!({
            "action":"train_merge", "id":"cas-train", "task_ids":[appended.id],
        })).unwrap();
        let start = Instant::now();
        let mut append = tokio::spawn(async move { service.factory(Parameters(request)).await });
        let response = tokio::time::timeout(Duration::from_secs(10), &mut append).await;
        let elapsed = start.elapsed();
        let contacted_origin = started.exists();
        // Always release and join the real subprocess path before asserting,
        // including the deliberately failing baseline.
        writeln!(release, "continue").unwrap();
        let timely = response.is_ok();
        let result = match response {
            Ok(result) => result,
            Err(_) => append.await,
        }.unwrap().unwrap();
        let queued = read(&queue_path).unwrap().unwrap();
        assert!(queued.revision > before.revision);
        assert_eq!(queued.next, before.next);
        assert_eq!(queued.entries.len(), 2);
        assert_eq!(queued.entries[1].task, appended.id);
        assert_eq!(queued.entries[1].sha, fixture.shas[1]);
        eprintln!("append elapsed={elapsed:?}; contacted_origin={contacted_origin}; revision={}; position=2", queued.revision);
        assert!(timely && !contacted_origin, "append waited on advancement: {elapsed:?}");
        assert_ne!(result.is_error, Some(true), "{result:?}");
        assert!(cas_store::get_latest_verification_dispatch(&fixture.root, &appended.id).unwrap().is_none());

        // The durable queue still starts the appended review on its existing
        // daemon pass; no follow-up supervisor request is required.
        resume_pending(&fixture.root, "queue-session").await.unwrap();
        assert!(started.exists());
        assert!(cas_store::get_latest_verification_dispatch(&fixture.root, &appended.id).unwrap().is_some());
    }

    /// A stalled later review must not reserve the entire train indefinitely.
    /// Replay publication, a held dispatch, its approval, an append and resume
    /// while the retired driver's OS handle still exists.
    #[tokio::test]
    async fn stalled_advancement_recovers_approved_head_and_appends_cas_deb2() {
        for holder in ["expired", "orphaned", "live"] {
            let mut fixture = Fixture::build(false);
            fixture._env.set("CAS_FACTORY_WORKER_CLI", "claude");
            fixture._env.set("CAS_FACTORY_SUPERVISOR_CLI", "claude");
            std::fs::write(fixture.root.join("config.toml"),
                "[verification]\nenabled=true\n[qa]\nmode='batch'\n").unwrap();
            let tasks = fixture.core.open_task_store().unwrap();
            let mut held = tasks.get(&fixture.ids[1]).unwrap();
            held.depth = TaskDepth::Deep;
            tasks.update(&held).unwrap();
            fixture.start_with(&fixture.ids[..2]).await;
            resume_pending(&fixture.root, "queue-session").await.unwrap();
            let queue_path = path(&fixture.root, fixture._repo.path(), "epic/train").unwrap();
            let before = read(&queue_path).unwrap().unwrap();
            assert_eq!(before.next, 1);
            let dispatch = cas_store::get_latest_verification_dispatch(&fixture.root, &held.id)
                .unwrap().unwrap();
            assert!(matches!(dispatch.state,
                VerificationDispatchState::Pending | VerificationDispatchState::Claimed));
            // Retain the historical long-lived flock, just as the incident's
            // blocked daemon does. New queue metadata operations must not wait
            // for this unrelated, unbounded legacy operation to return.
            let common = GitOperations::new(fixture._repo.path().to_path_buf()).canonical_repo_key().unwrap();
            let legacy = target_lock::try_lock_delivery_target(
                &fixture.root, &common, "train-merge-driver/epic/train",
            ).unwrap().unwrap();
            let mut exited = std::process::Command::new("true").spawn().unwrap();
            let dead_pid = exited.id();
            exited.wait().unwrap();
            let now = chrono::Utc::now();
            let mut persisted: serde_json::Value = serde_json::from_slice(
                &std::fs::read(&queue_path).unwrap()).unwrap();
            persisted["advancement"] = serde_json::json!({
                "token": "retired-advance", "pid": if holder == "orphaned" { dead_pid } else { std::process::id() },
                "expires_at": now + chrono::Duration::seconds(if holder == "expired" { -1 } else { 120 }),
            });
            std::fs::write(&queue_path, serde_json::to_vec(&persisted).unwrap()).unwrap();
            let service = crate::mcp::CasService::new(fixture.core.clone(), None);
            let result = service.verification(Parameters(serde_json::from_value(serde_json::json!({
                "action":"add", "task_id":held.id, "dispatch_id":dispatch.id,
                "status":"approved", "summary":"Review the held train candidate",
            })).unwrap())).await.unwrap();
            assert_ne!(result.is_error, Some(true), "{result:?}");
            let mut stale_snapshot = read(&queue_path).unwrap().unwrap();
            fixture.start_with(&fixture.ids[2..]).await;
            assert!(save(&queue_path, &mut stale_snapshot).is_err(), "{holder}: stale driver overwrote append");
            let appended = read(&queue_path).unwrap().unwrap();
            assert_eq!(appended.entries.len(), 3, "{holder}");
            assert_eq!(appended.next, 1, "{holder}");
            let persisted: serde_json::Value = serde_json::from_slice(
                &std::fs::read(&queue_path).unwrap()).unwrap();
            if holder == "live" {
                assert_eq!(persisted["advancement"]["token"], "retired-advance");
                // Completion by that live owner releases its lease; appending
                // above must neither steal it nor discard the added entry.
                let mut persisted = persisted;
                persisted["advancement"] = serde_json::Value::Null;
                std::fs::write(&queue_path, serde_json::to_vec(&persisted).unwrap()).unwrap();
            } else {
                assert!(persisted["advancement"].is_null(), "{holder}");
                assert_eq!(persisted["advancement_reclaims"].as_array().unwrap().len(), 1, "{holder}");
            }
            drop(legacy);
            resume_pending(&fixture.root, "queue-session").await.unwrap();
            let complete = read(&queue_path).unwrap().unwrap();
            assert!(!complete.active(), "{holder}: {}", complete.render());
            let git = GitOperations::new(fixture._repo.path().to_path_buf());
            for sha in &fixture.shas {
                assert!(git.is_ancestor(sha, "origin/epic/train"), "{holder}: {sha}");
            }
        }
    }

    #[tokio::test]
    async fn train_merge_returns_conflict_and_preserves_order_across_resume() {
        let fixture = Fixture::new();
        fixture.start().await;
        let ci_dir = crate::train_ci::directory(&fixture.root, "cas-train").unwrap();
        std::fs::create_dir_all(&ci_dir).unwrap();
        let mut red = serde_json::json!({"branch":"epic/train", "head":"bad-tip", "red_head":"bad-tip",
            "red_since":"bad-tip", "run_id":"17", "run_attempt":1, "outcome":"FAIL", "failures":["fixture::regression"],
            "complete_log":"complete-workflow-logs.zip"});
        std::fs::write(ci_dir.join("state.json"), red.to_string()).unwrap();
        resume_pending(&fixture.root, "queue-session")
            .await
            .unwrap();
        assert!(
            !GitOperations::new(fixture._repo.path().to_path_buf())
                .is_ancestor(&fixture.shas[0], "epic/train"),
            "a running queue must hold on red CI"
        );
        let service = crate::mcp::CasService::new(fixture.core.clone(), None);
        let request = |override_ci, reason| {
            serde_json::from_value(serde_json::json!({
            "action":"train_merge", "id":"cas-train", "supervisor_override":override_ci, "reason":reason,
        })).unwrap()
        };
        let _ = service
                .factory(Parameters(request(false, None::<&str>)))
                .await
                .unwrap_err()
                .message;
        let _ = service
                .factory(Parameters(request(true, None)))
                .await
                .unwrap_err()
                .message;
        service
            .factory(Parameters(request(true, Some("merge the reviewed repair"))))
            .await
            .unwrap();
        assert!(
            fixture
                .core
                .open_task_store()
                .unwrap()
                .get("cas-train")
                .unwrap()
                .notes
                .contains("TRAIN SUITE OVERRIDE")
        );
        // Exercise the ordinary merge entry, rather than just the reservation
        // helper. Singles cannot overtake the queued first delivery.
        let error = fixture
            .core
            .worktree_merge(
                "factory/worker-3",
                false,
                Some(&fixture.ids[2]),
                false,
                None,
                false,
                None,
            )
            .await
            .unwrap_err();

        let operations = GitOperations::new(fixture._repo.path().to_path_buf());
        assert!(!operations.is_ancestor(&fixture.shas[2], "epic/train"));
        // A wrong session cannot consume another factory's intent.
        resume_pending(&fixture.root, "other-session")
            .await
            .unwrap();
        assert!(!operations.is_ancestor(&fixture.shas[0], "epic/train"));
        let dispatch = cas_store::create_verification_dispatch(
            &fixture.root,
            &fixture.ids[0],
            "supervisor",
            "supervisor",
            chrono::Utc::now() + chrono::Duration::minutes(10),
        )
        .unwrap();
        resume_pending(&fixture.root, "queue-session")
            .await
            .unwrap();
        let held = read(&path(&fixture.root, fixture._repo.path(), "epic/train").unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(
            held.next, 0,
            "pending verdict must retain the first position"
        );
        assert!(!operations.is_ancestor(&fixture.shas[0], "epic/train"));
        assert!(
            held.waiting
                .as_deref()
                .is_some_and(|message| message.contains(&dispatch.id)),
            "the exact red override must allow the queue to reach review"
        );
        let service = crate::mcp::CasService::new(fixture.core.clone(), None);
        let request = serde_json::from_value(serde_json::json!({
            "action": "add", "task_id": fixture.ids[0], "dispatch_id": dispatch.id,
            "status": "approved", "summary": "Resume the queued exact delivery",
        }))
        .unwrap();
        let approval = service.verification(Parameters(request)).await.unwrap();
        assert_ne!(approval.is_error, Some(true), "{approval:?}");
        // A rerun attempt is not covered by the earlier override. Green then
        // releases the same durable position without another override.
        red["run_attempt"] = 2.into();
        std::fs::write(ci_dir.join("state.json"), red.to_string()).unwrap();
        resume_pending(&fixture.root, "queue-session")
            .await
            .unwrap();
        assert!(!operations.is_ancestor(&fixture.shas[0], "epic/train"));
        red["outcome"] = "PASS".into();
        red.as_object_mut().unwrap().remove("red_since");
        std::fs::write(ci_dir.join("state.json"), red.to_string()).unwrap();
        // Reconstruct the consumer from durable state, as after daemon restart.
        resume_pending(&fixture.root, "queue-session")
            .await
            .unwrap();
        let queue = read(&path(&fixture.root, fixture._repo.path(), "epic/train").unwrap())
            .unwrap()
            .unwrap();
        assert!(!queue.active(), "{}", queue.render());
        assert!(operations.is_ancestor(&fixture.shas[0], "epic/train"));
        assert!(!operations.is_ancestor(&fixture.shas[1], "epic/train"));
        assert!(operations.is_ancestor(&fixture.shas[2], "epic/train"));
        let returned = fixture
            .core
            .open_task_store()
            .unwrap()
            .get(&fixture.ids[1])
            .unwrap();
        assert_eq!(returned.status, TaskStatus::Open);
        assert_eq!(returned.assignee.as_deref(), Some("worker-2"));
        assert!(returned.notes.contains("shared.md"), "{}", returned.notes);
        assert_eq!(
            queue
                .outcomes
                .iter()
                .map(|s| s.split_whitespace().next().unwrap())
                .collect::<Vec<_>>(),
            fixture.ids.iter().map(String::as_str).collect::<Vec<_>>()
        );
        assert_eq!(
            git(fixture._repo.path(), &["rev-parse", "epic/train^2"]),
            fixture.shas[2]
        );
        assert!(operations.is_ancestor(&fixture.shas[0], "epic/train^1"));
        admit(
            &fixture.core,
            fixture._repo.path(),
            "epic/train",
            None,
            None,
        )
        .unwrap();
        let before = git(fixture._repo.path(), &["rev-parse", "epic/train"]);
        resume_pending(&fixture.root, "queue-session")
            .await
            .unwrap();
        assert_eq!(
            git(fixture._repo.path(), &["rev-parse", "epic/train"]),
            before,
            "completed queue is idempotent"
        );
    }

    /// GH #2018: a train entry that needs a verdict asks the factory daemon to
    /// start its verifier during the resume pass. A manual handoff instead
    /// named worktree_merge, which G-101 refuses while the train owns the target.
    /// GH #2169: with three queued deliveries, one pass starts every verifier
    /// before the first publish, and publication still follows queue order.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn resume_requests_daemon_verifier_for_train_entry_gh_2018() {
        use crate::ui::factory::{
            ClientMessage, DaemonMessage, FRAME_HEADER_SIZE, PROTOCOL_VERSION, SessionState,
            decode_length, encode_message, gui_socket_path, local_gui_capabilities,
        };
        use std::io::{Read, Write};
        let mut fixture = Fixture::build(false);
        fixture._env.set("CAS_FACTORY_WORKER_CLI", "claude");
        fixture._env.set("CAS_FACTORY_SUPERVISOR_CLI", "claude");
        std::fs::write(
            fixture.root.join("config.toml"),
            "[verification]\nenabled=true\n[qa]\nmode='batch'\n",
        )
        .unwrap();
        let tasks = fixture.core.open_task_store().unwrap();
        // GH #2169: three independent deliveries; every verifier starts at once.
        for id in &fixture.ids {
            let mut task = tasks.get(id).unwrap();
            task.depth = TaskDepth::Deep;
            tasks.update(&task).unwrap();
        }
        // Enqueue starts its reviews; an active append acknowledges its saved
        // position without preparing a candidate until the daemon resumes it.
        let dispatch_exists = |id: &String| cas_store::get_latest_verification_dispatch(&fixture.root, id).unwrap().is_some();
        fixture.start_with(&fixture.ids[..2]).await;
        assert!(fixture.ids[..2].iter().all(dispatch_exists), "enqueue starts the queued reviews");
        assert!(!dispatch_exists(&fixture.ids[2]));
        fixture.start().await;
        assert!(!dispatch_exists(&fixture.ids[2]));
        // The live stuck state: a daemonless pass left this exact dispatch
        // Pending and unclaimed.
        let parked = tasks.get(&fixture.ids[0]).unwrap();
        assert!(fixture.core.prepare_premerge_verification(
            &parked, fixture._repo.path(), "factory/worker-1", "epic/train",
        ).is_err());
        let pending = cas_store::get_latest_verification_dispatch(&fixture.root, &fixture.ids[0])
            .unwrap()
            .unwrap();
        assert_eq!(pending.state, VerificationDispatchState::Pending);
        let broker_root = fixture.root.clone();
        // The factory daemon's local control socket, as the real daemon serves it.
        let socket = gui_socket_path("queue-session");
        std::fs::create_dir_all(socket.parent().unwrap()).unwrap();
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        listener.set_nonblocking(true).unwrap();
        let wanted = fixture.ids.len();
        let broker = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
            let mut clients = Vec::new();
            while clients.len() < wanted && std::time::Instant::now() < deadline {
                match listener.accept() {
                    Ok((client, _)) => {
                        let broker_root = broker_root.clone();
                        clients.push(std::thread::spawn(move || {
                            let mut client = client;
                            client.set_nonblocking(false).unwrap();
                            client
                                .write_all(&encode_message(&DaemonMessage::Welcome {
                                    session_name: "queue-session".into(),
                                    state: SessionState { focused_pane: None, panes: vec![], epic_id: None,
                                        epic_title: None, cols: 80, rows: 24 },
                                    scrollback: None, protocol_version: PROTOCOL_VERSION,
                                    capabilities: local_gui_capabilities(), pane_bootstrap: vec![],
                                }).unwrap())
                                .unwrap();
                            let mut header = [0; FRAME_HEADER_SIZE];
                            client.read_exact(&mut header).unwrap();
                            let mut body = vec![0; decode_length(&header)];
                            client.read_exact(&mut body).unwrap();
                            let ClientMessage::LaunchVerifier { dispatch_id } = serde_json::from_slice(&body).unwrap()
                            else {
                                panic!("unexpected factory control");
                            };
                            // The real daemon launch path claims the exact dispatch.
                            let review = crate::mcp::tools::core::workflow::premerge_verification::launch_factory_verifier(
                                &broker_root, "queue-session", &dispatch_id,
                            )
                            .unwrap();
                            let claimed = cas_store::get_verification_dispatch(&broker_root, &dispatch_id).unwrap();
                            client
                                .write_all(&encode_message(&DaemonMessage::VerifierLaunched {
                                    dispatch_id: dispatch_id.clone(),
                                }).unwrap())
                                .unwrap();
                            if let Some(review) = review {
                                review.join().unwrap();
                            }
                            (dispatch_id, claimed.state, claimed.verifier_agent_id)
                        }));
                    }
                    Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
                }
            }
            clients.into_iter().map(|client| client.join().unwrap()).collect::<Vec<_>>()
        });

        resume_pending(&fixture.root, "queue-session").await.unwrap();

        let dispatches: Vec<_> = fixture.ids.iter().map(|id| {
            cas_store::get_latest_verification_dispatch(&fixture.root, id)
                .unwrap()
                .expect("every queued delivery has its own dispatch before any publish")
        }).collect();
        let launches = broker.join().unwrap();
        // GH #2169: one resume pass started all three verifiers, and each claimed its own dispatch.
        let mut launched: Vec<_> = launches.iter().map(|(id, _, _)| id.clone()).collect();
        launched.sort();
        let mut expected: Vec<_> = dispatches.iter().map(|d| d.id.clone()).collect();
        expected.sort();
        assert_eq!(launched, expected);
        assert!(launches.iter().all(|(_, state, _)| *state == VerificationDispatchState::Claimed));
        assert!(launches.iter().all(|(_, _, verifier)| verifier.as_ref().is_some_and(|id| id.starts_with("task-verifier-"))));
        assert_eq!(dispatches[0].id, pending.id);
        let operations = GitOperations::new(fixture._repo.path().to_path_buf());
        for sha in &fixture.shas {
            assert!(!operations.is_ancestor(sha, "epic/train") && !operations.is_ancestor(sha, "origin/epic/train"));
        }
        // Verdicts arrive out of order: nothing publishes ahead of the head.
        let service = crate::mcp::CasService::new(fixture.core.clone(), None);
        let approve = |index: usize| {
            let request = serde_json::from_value(serde_json::json!({
                "action": "add", "task_id": fixture.ids[index], "dispatch_id": dispatches[index].id,
                "status": "approved", "summary": "Exact train candidate reviewed",
            }))
            .unwrap();
            service.verification(Parameters(request))
        };
        for index in [2, 1] {
            let approval = approve(index).await.unwrap();
            assert_ne!(approval.is_error, Some(true), "{approval:?}");
        }
        resume_pending(&fixture.root, "queue-session").await.unwrap();
        for sha in &fixture.shas {
            assert!(!operations.is_ancestor(sha, "origin/epic/train"), "published ahead of the head");
        }
        // The head's verdict releases all three, in queue order.
        let approval = approve(0).await.unwrap();
        assert_ne!(approval.is_error, Some(true), "{approval:?}");
        resume_pending(&fixture.root, "queue-session").await.unwrap();
        for sha in &fixture.shas {
            assert!(operations.is_ancestor(sha, "origin/epic/train"));
        }
        let train = |rev: &str| git(fixture._repo.path(), &["rev-parse", rev]);
        assert_eq!(
            [train("origin/epic/train^2"), train("origin/epic/train^1^2"), train("origin/epic/train^1^1^2")],
            [fixture.shas[2].clone(), fixture.shas[1].clone(), fixture.shas[0].clone()],
        );
    }

    /// GH #2017: a delivery still parked on a train whose branch is gone from
    /// origin must not stop another train's merge from advancing.
    #[tokio::test]
    async fn parked_task_on_deleted_train_does_not_stall_other_train_gh_2017() {
        let fixture = Fixture::build(false);
        fixture.start().await;
        git(fixture._repo.path(), &["branch", "epic/deleted", "main"]);
        let config = fixture.root.join("config.toml");
        let pinned = std::fs::read_to_string(&config).unwrap() + "[project]\ncanonical_id = 'gh-2017-fixture'\n";
        std::fs::write(&config, pinned).unwrap();
        let tasks = fixture.core.open_task_store().unwrap();
        let mut stale_train = Task::new("cas-stale-train".into(), "cancelled train".into());
        stale_train.task_type = TaskType::Epic;
        stale_train.branch = Some("epic/deleted".into());
        tasks.add(&stale_train).unwrap();
        let mut stale = Task::new("cas-stale-delivery".into(), "parked on a deleted train".into());
        stale.status = TaskStatus::AwaitingMerge;
        stale.deliverables.factory_branch_anchor = Some(fixture.shas[2].clone());
        stale.deliverables.parked_branch = Some("factory/worker-3".into());
        stale.deliverables.work_target = Some(cas_types::WorkTarget {
            repo_selector: "project:gh-2017-fixture".into(),
            target_branch: "epic/deleted".into(),
        });
        tasks.create_atomic(&stale, &[], Some(&stale_train.id), None).unwrap();

        resume_pending(&fixture.root, "queue-session").await.unwrap();

        let queue = read(&path(&fixture.root, fixture._repo.path(), "epic/train").unwrap())
            .unwrap()
            .unwrap();
        assert!(!queue.active(), "{}", queue.render());
        assert_eq!(tasks.get(&stale.id).unwrap().status, TaskStatus::AwaitingMerge);
        let unchecked = crate::mcp::tools::core::task::lifecycle::parked_conflicts::scan(&fixture.root).unwrap();
        assert!(unchecked.contains_key("epic/deleted"), "{unchecked:?}");
    }

    /// A train asks origin about its target once per entry: under the merge
    /// lock, shared by containment, the target sync, publication and the
    /// close's integration proof.
    #[tokio::test]
    async fn train_merge_reads_origin_once_per_entry() {
        let fixture = Fixture::build(false);
        fixture.start().await;
        let reads = fixture.root.join("origin-reads.log");
        let counter = fixture.root.join("count-upload-pack");
        std::fs::write(&counter, format!(
            "#!/bin/sh\nprintf '%s | %s\\n' \"$(ps -o args= -p $PPID)\" \"$(ps -o args= -p \"$(ps -o ppid= -p $PPID | tr -d ' ')\")\" >> '{}'\nexec git upload-pack \"$@\"\n",
            reads.display(),
        )).unwrap();
        std::fs::set_permissions(&counter, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        git(fixture._repo.path(), &["config", "remote.origin.uploadpack", counter.to_str().unwrap()]);

        resume_pending(&fixture.root, "queue-session").await.unwrap();

        let queue = read(&path(&fixture.root, fixture._repo.path(), "epic/train").unwrap())
            .unwrap()
            .unwrap();
        assert!(!queue.active(), "{}", queue.render());
        let operations = GitOperations::new(fixture._repo.path().to_path_buf());
        for sha in &fixture.shas {
            assert!(operations.is_ancestor(sha, "origin/epic/train"), "{:?}", queue.outcomes);
        }
        let reads = std::fs::read_to_string(&reads).unwrap_or_default();
        assert_eq!(reads.lines().count(), fixture.ids.len(), "origin reads:\n{reads}");
    }
}
