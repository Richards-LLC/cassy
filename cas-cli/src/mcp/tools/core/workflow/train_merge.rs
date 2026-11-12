//! Ordered supervisor merge intents. The daemon advances one delivery at a
//! time through normal merge admission; a pending verdict retains its position.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use cas_types::{AgentRole, TaskStatus, TaskType, VerificationDispatchState};
use serde::{Deserialize, Serialize};

use crate::mcp::CasCore;
use crate::worktree::{GitOperations, target_lock};

#[derive(Debug, Serialize, Deserialize)]
struct Entry {
    task: String,
    branch: String,
    sha: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Queue {
    id: String,
    train: String,
    repo: PathBuf,
    target: String,
    supervisor: String,
    session: String,
    entries: Vec<Entry>,
    next: usize,
    outcomes: Vec<String>,
    waiting: Option<String>,
    #[serde(default)]
    ci_override: Option<String>,
}

impl Queue {
    fn active(&self) -> bool {
        self.next < self.entries.len()
    }

    fn render(&self) -> String {
        format!(
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
        )
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

fn save(path: &Path, queue: &Queue) -> Result<()> {
    let parent = path.parent().context("queue directory missing")?;
    std::fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    serde_json::to_writer(std::io::Write::by_ref(&mut file), queue)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

/// Checked before preparing a candidate and again under the normal target
/// lock. The queue creates its reservation under that same lock, so a single
/// merge already in flight cannot slip between reservation and publication.
// gate[G-101 merge-admission integrity]: Train merge active reservation
pub(crate) fn admit(
    root: &Path,
    repo: &Path,
    target: &str,
    task: Option<&str>,
    permit: Option<&str>,
) -> Result<()> {
    let queue = read(&path(root, repo, target)?)?;
    if let Some(queue) = queue.filter(Queue::active) {
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

fn driver_lock(
    root: &Path,
    repo: &Path,
    target: &str,
) -> Result<Option<target_lock::DeliveryTargetLock>> {
    let common = GitOperations::new(repo.to_path_buf()).canonical_repo_key()?;
    Ok(target_lock::try_lock_delivery_target(
        root,
        &common,
        &format!("train-merge-driver/{target}"),
    )?)
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
        let _driver = driver_lock(&self.cas_root, &repo, &target)?
            .context(crate::gates::tag("G-113", "train merge is being advanced; retry this request"))?;
        let path = path(&self.cas_root, &repo, &target)?;
        if let Some(mut queue) = read(&path)?.filter(Queue::active) {
            ensure!(queue.train == train, crate::gates::tag("G-113", "another train owns this target"));
            if let Some(ordered) = ordered {
                ensure!(
                    ordered
                        .iter()
                        .map(String::as_str)
                        .eq(queue.entries.iter().map(|e| e.task.as_str())),
                    crate::gates::tag("G-113", "active train merge has a different ordered list; resume without task_ids")
                );
            }
            if queue.supervisor != actor.id {
                let agents = self
                    .open_agent_store()
                    .map_err(|e| anyhow::anyhow!(e.to_string()))?;
                ensure!(
                    !agents.get(&queue.supervisor).is_ok_and(|a| a.is_alive()),
                    crate::gates::tag("G-113", "another live supervisor owns this train merge")
                );
                queue.supervisor = actor.id;
            }
            let prior_override = queue.ci_override.clone();
            queue.ci_override = crate::train_ci::guard(
                self,
                train,
                &target,
                queue.ci_override.as_deref(),
                supervisor_override,
                reason,
            )?;
            queue.session = session.into();
            save(&path, &queue)?;
            let receipt = queue.render();
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
        let mut entries = Vec::new();
        let mut seen = std::collections::BTreeSet::new();
        for id in ids {
            ensure!(seen.insert(id.clone()), crate::gates::tag("G-113", format!("duplicate train delivery {id}")));
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
                &agents,
                Some(session),
                task.deliverables.parked_branch.as_deref().unwrap_or(""),
                Some(&task),
                chrono::Utc::now(),
            ) {
                anyhow::bail!(crate::gates::tag("G-93", owner.refusal("train_merge", session)));
            }
            if let Some(work) = task.deliverables.work_target.as_ref() {
                let context =
                    super::super::task::repo_context::resolve_repo_context(&self.cas_root, work)
                        .map_err(anyhow::Error::msg)?;
                ensure!(
                    context.target_branch == target
                        && context.git_common_dir
                            == GitOperations::new(repo.clone()).canonical_repo_key()?,
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
        let queue = Queue {
            id: uuid::Uuid::new_v4().to_string(),
            train: train.into(),
            repo: repo.clone(),
            target: target.clone(),
            supervisor: actor.id,
            session: session.into(),
            entries,
            next: 0,
            outcomes: vec![],
            waiting: None,
            ci_override,
        };
        let common = GitOperations::new(repo).canonical_repo_key()?;
        let _target = target_lock::try_lock_delivery_target(&self.cas_root, &common, &target)?
            .context(crate::gates::tag("G-113", "target merge already in progress; retry train_merge after it finishes"))?;
        save(&path, &queue)?;
        let receipt = queue.render();
        Ok(if queue.ci_override.is_some() { crate::gates::override_tag("G-114", receipt) } else { receipt })
    }
}

fn published(queue: &Queue, entry: &Entry) -> Result<bool> {
    let git = GitOperations::new(queue.repo.clone());
    // Publication, rather than a local-only merge, advances the queue.
    let target = git
        .fetch_merge_target(&queue.target)?
        .or_else(|| git.resolve_commit(&queue.target))
        .context("train target missing")?;
    Ok(git.is_ancestor(&entry.sha, &target))
}

/// Existing daemon scan, off its control loop. No additional CI stage or
/// model turn is needed to consume an approved candidate.
// gate-audit[ci_override]: selector Derived accepted G-114 token forwarded in response; not another request flag.
pub(crate) async fn resume_pending(root: &Path, session: &str) -> Result<()> {
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
        let Some(_driver) = driver_lock(root, &queue.repo, &queue.target)? else {
            continue;
        };
        let Some(mut queue) = read(&path)? else {
            continue;
        };
        let core = CasCore::with_daemon(root.to_path_buf(), None, None);
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
        while queue.active() {
            if let Err(error) = crate::train_ci::guard(
                &core,
                &queue.train,
                &queue.target,
                queue.ci_override.as_deref(),
                false,
                None,
            ) {
                queue.waiting = Some(error.to_string());
                save(&path, &queue)?;
                break;
            }
            let entry = &queue.entries[queue.next];
            if published(&queue, entry)? {
                queue
                    .outcomes
                    .push(format!("{} merged {}", entry.task, entry.sha));
                queue.next += 1;
                queue.waiting = None;
                save(&path, &queue)?;
                continue;
            }
            let task = tasks.get(&entry.task)?;
            // gate[G-115 train-merge integrity]: Train entry awaiting dispatch or returned entry skip
            if task.status != TaskStatus::AwaitingMerge
                || task.deliverables.factory_branch_anchor.as_deref() != Some(&entry.sha)
                || task.deliverables.parked_branch.as_deref() != Some(&entry.branch)
            {
                queue.outcomes.push(crate::gates::tag("G-115", format!(
                    "{} returned or changed; skipped exact parked delivery",
                    entry.task
                )));
                queue.next += 1;
                queue.waiting = None;
                save(&path, &queue)?;
                continue;
            }
            if let Some(dispatch) = cas_store::get_latest_verification_dispatch(root, &entry.task)?
                && matches!(
                    dispatch.state,
                    VerificationDispatchState::Pending | VerificationDispatchState::Claimed
                )
            {
                queue.waiting = Some(crate::gates::tag("G-115", format!("{} awaiting dispatch {}", entry.task, dispatch.id)));
                save(&path, &queue)?;
                break;
            }
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
            };
            let result = core.worktree_merge_queued(&expected, &queue.id).await;
            let current = tasks.get(&entry.task)?;
            if current.status != TaskStatus::AwaitingMerge || published(&queue, entry)? {
                // Re-read the durable result at the top, including a conflict
                // return or a merge committed just before a previous crash.
                continue;
            }
            queue.waiting = Some(match result {
                Err(error) => error.message.to_string(),
                Ok(result) => result
                    .content
                    .iter()
                    .filter_map(|c| match &c.raw {
                        rmcp::model::RawContent::Text(text) => Some(text.text.as_str()),
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            });
            save(&path, &queue)?;
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
                shas[index] = if index < 2 {
                    commit(&checkout, "shared.md", &format!("delivery {}\n", index + 1))
                } else {
                    commit(&checkout, "third.md", "third delivery\n")
                };
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
            let service = crate::mcp::CasService::new(self.core.clone(), None);
            let request = serde_json::from_value(serde_json::json!({
                "action": "train_merge", "id": "cas-train", "task_ids": self.ids
            }))
            .unwrap();
            let result = service.factory(Parameters(request)).await.unwrap();
            assert_ne!(result.is_error, Some(true), "{result:?}");
        }
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
        // Count origin reads: merge admission asks origin once per entry.
        let reads = fixture.root.join("origin-reads.log");
        let counter = fixture.root.join("count-upload-pack");
        std::fs::write(&counter, format!("#!/bin/sh\necho read >> '{}'\nexec git upload-pack \"$@\"\n", reads.display())).unwrap();
        std::fs::set_permissions(&counter, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        git(fixture._repo.path(), &["config", "remote.origin.uploadpack", counter.to_str().unwrap()]);
        // Reconstruct the consumer from durable state, as after daemon restart.
        resume_pending(&fixture.root, "queue-session")
            .await
            .unwrap();
        let reads = std::fs::read_to_string(&reads).unwrap_or_default().lines().count();
        assert_eq!(reads, fixture.ids.len(), "origin reads for {} train entries", fixture.ids.len());
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
            &fixture.root,
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
}
