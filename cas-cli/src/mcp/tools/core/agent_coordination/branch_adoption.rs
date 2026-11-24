//! Opt-in branch handoff preserving a receiving worker's existing delivery ancestry.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use cas_types::{Agent, AgentRole, Task};

use crate::bounded_process::{Deadline, run_command};
use crate::factory_isolation::{
    WorkerBinding, classify_worker_binding, verify_worker_worktree_binding, worker_task_branch,
};
use crate::mcp::tools::core::task::lifecycle::close_ops;
use crate::store::{open_task_store, open_worktree_store};

pub(super) struct AdoptedBranch {
    pub source_branch: String,
    pub branch: String,
    pub tip: String,
    /// Start boundary of the adopted delivery: the recorded base when the
    /// delivery still descends from it, else the delivery's fork from target.
    pub base: Option<String>,
}

struct Git<'a> {
    root: &'a Path,
    deadline: Deadline,
}

impl Git<'_> {
    fn output(&self, args: &[&str]) -> Result<Output, String> {
        run_command(
            // cas-39f3: run_command SIGKILLs at the deadline; without
            // optional locks a killed `status` cannot strand index.lock.
            Command::new("git")
                .env("GIT_OPTIONAL_LOCKS", "0")
                .arg("-C")
                .arg(self.root)
                .args(args),
            self.deadline,
            Duration::from_secs(5),
        )
        .map_err(|error| format!("git {args:?} failed: {error:?}"))
    }

    fn run(&self, args: &[&str]) -> Result<String, String> {
        let output = self.output(args)?;
        if !output.status.success() {
            return Err(format!(
                "git {args:?}: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn ancestor(&self, before: &str, after: &str) -> Result<bool, String> {
        let output = self.output(&["merge-base", "--is-ancestor", before, after])?;
        match output.status.code() {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err("cannot prove fast-forward ancestry".into()),
        }
    }

    fn common_dir(&self) -> Result<PathBuf, String> {
        let path = self.run(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
        PathBuf::from(path)
            .canonicalize()
            .map_err(|e| e.to_string())
    }

    fn require_integrated(
        &self,
        cas_root: &Path,
        repo_root: &Path,
        task: &Task,
        head: &str,
        label: &str,
    ) -> Result<(), String> {
        let parent = open_task_store(cas_root)
            .and_then(|store| Ok(store.get_parent_epic(&task.id)?))
            .map_err(|error| format!("cannot resolve task integration target: {error}"))?;
        let target = match close_ops::effective_close_work_target(task, parent.as_ref()) {
            Some(target) => target.target_branch,
            None => {
                let worktree_parent = task.worktree_id.as_deref().map(|id| {
                    open_worktree_store(cas_root)
                        .and_then(|store| Ok(store.get(id)?))
                        .map(|worktree| worktree.parent_branch)
                        .map_err(|error| format!("cannot resolve task worktree target: {error}"))
                }).transpose()?;
                close_ops::resolve_close_parent_branch(
                    worktree_parent,
                    parent.as_ref().and_then(|epic| epic.branch.clone()),
                    None,
                    repo_root,
                )?
            }
        };
        let target_ref = close_ops::preferred_diff_target_ref_for_delivery(
            self.root, &target, task.delivery_mode,
        );
        if !self.ancestor(head, &target_ref)? {
            return Err(format!("{label} contains commits outside the integration target {target_ref}; preserve or integrate that work before adopting this delivery"));
        }
        Ok(())
    }
}

/// The newest first-parent commit after `anchor` on `head` such that every
/// commit up to it names `task_id` in its subject (`fix(<task-id>): ...`).
fn task_rework_tip(git: &Git<'_>, anchor: &str, head: &str, task_id: &str) -> Result<String, String> {
    let names_task = |subject: &str| subject.match_indices(task_id).any(|(at, _)| {
        let before = subject[..at].chars().next_back();
        let after = subject[at + task_id.len()..].chars().next();
        !before.is_some_and(|c| c.is_ascii_alphanumeric() || c == '-')
            && !after.is_some_and(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
    });
    let mut tip = anchor.to_string();
    let range = format!("{anchor}..{head}");
    for line in git.run(&["log", "--reverse", "--first-parent", "--format=%H %s", "--end-of-options", &range])?.lines() {
        let Some((commit, subject)) = line.split_once(' ') else { break };
        if !names_task(subject) {
            break;
        }
        tip = commit.to_string();
    }
    Ok(tip)
}

pub(super) fn adopt_task_branch(
    cas_root: &Path,
    task: &Task,
    receiver: &Agent,
) -> Result<AdoptedBranch, String> {
    if task.is_terminal() || receiver.role != AgentRole::Worker {
        return Err("only a nonterminal task can adopt a registered worker's branch".into());
    }
    // cas-2d20: an isolated worker's registration can lack clone_path; its
    // managed worktree (`<worktree base>/<name>`) is then the one Cassy
    // provisioned for it. Either way the binding check below must pass.
    let destination: PathBuf = receiver
        .metadata
        .get("clone_path")
        .map(|path| path.trim())
        .filter(|path| !path.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            crate::mcp::tools::core::task::lifecycle::close_ops::resolve_system_b_worktree_path(
                cas_root,
                &receiver.name,
            )
        })
        .ok_or(
            "receiver has no registered clone_path and no managed worktree; spawn an isolated \
             worker first",
        )?;
    let destination = destination.as_path();
    let deadline = Deadline::after(Duration::from_secs(20));
    let git = Git {
        root: destination,
        deadline,
    };
    let repo_root = if let Some(target) = &task.deliverables.work_target {
        super::super::task::repo_context::resolve_repo_context(cas_root, target)?.repo_root
    } else {
        cas_root
            .parent()
            .ok_or("missing project repository")?
            .to_path_buf()
    };
    if git.common_dir()?
        != (Git {
            root: &repo_root,
            deadline,
        })
        .common_dir()?
    {
        return Err("receiver worktree belongs to a different repository than the task".into());
    }
    if !git.run(&["status", "--porcelain"])?.is_empty() {
        return Err("receiver worktree has uncommitted work; commit it before adopting".into());
    }
    let mut candidates: Vec<String> = task
        .assignee
        .as_deref()
        .map(|prior| worker_task_branch(prior, &task.id))
        .into_iter()
        .collect();
    // Reboot cleanup can clear the assignee while retaining the source
    // recorded at start. Adopt that exact branch without reviving its owner.
    if task.assignee.is_none() {
        candidates.extend(
            task.deliverables
                .factory_branch_start
                .as_ref()
                .map(|start| start.branch.clone()),
        );
    }
    candidates.extend(task.deliverables.parked_branch.iter().cloned());
    candidates.extend(task.deliverables.handoff_branches.iter().rev().cloned());
    if let Some(prior) = task.assignee.as_deref() {
        candidates.push(crate::factory_isolation::expected_worker_branch(prior));
    }
    let mut resolved = Vec::new();
    for branch in candidates.iter().filter(|branch| branch.starts_with("factory/")) {
        for prefix in ["refs/heads", "refs/remotes/origin"] {
            let reference = format!("{prefix}/{branch}^{{commit}}");
            let output = git.output(&["rev-parse", "--verify", "--end-of-options", &reference])?;
            if output.status.success() {
                resolved.push((
                    branch.clone(),
                    String::from_utf8_lossy(&output.stdout).trim().to_string(),
                ));
                break;
            }
        }
    }
    // cas-ea31 (takeover case 3): a review return moves the anchor to
    // history, and the prior worker's task branch can sit at the train base.
    // Adopt from the first candidate that holds the delivery, and never past
    // it: a lane can also carry a later task's commits.
    let delivery = match task.deliverables.factory_branch_anchor.as_ref()
        .or(task.deliverables.historical_factory_branch_anchors.last())
    {
        Some(anchor) => Some(git.run(&[
            "rev-parse", "--verify", "--end-of-options", &format!("{anchor}^{{commit}}"),
        ])?),
        None => None,
    };
    let mut holding = None;
    if let Some(delivery) = &delivery {
        for (branch, head) in &resolved {
            if git.ancestor(delivery, head)? {
                holding = Some((branch.clone(), head.clone()));
                break;
            }
        }
    }
    let (source_branch, source_head) = holding
        .or_else(|| resolved.first().cloned())
        .ok_or("no factory delivery branch found for this task")?;
    let tip = match delivery {
        Some(delivery) => {
            if !git.ancestor(&delivery, &source_head)? {
                return Err("task delivery anchor is not on the source branch".into());
            }
            if task.deliverables.factory_branch_anchor.is_some() {
                delivery
            } else {
                // A retired anchor (review return, conflict rework) can be
                // followed by this task's own rework. Adopt that rework, and
                // stop at the first commit that does not name this task.
                task_rework_tip(&git, &delivery, &source_head, &task.id)?
            }
        }
        None => source_head,
    };
    let current_branch = git.run(&["symbolic-ref", "--short", "HEAD"])?;
    // cas-a9fe, cas-ea31 (takeover case 2): a supervisor may repair a
    // receiver already checked out on any of this task's branches. Prove the
    // registered path is a real root and accept only those exact branches,
    // never an unrelated foreign branch.
    let foreign_source = candidates.contains(&current_branch)
        && classify_worker_binding(&receiver.name, Some(&current_branch)) != WorkerBinding::Own;
    verify_worker_worktree_binding(
        &receiver.name,
        destination,
        foreign_source.then_some(current_branch.as_str()),
    )
    .map_err(|e| e.to_string())?;
    // Copy the recorded anchor without moving an advanced foreign source.
    // A fresh receiver may already be on the integration target after this
    // delivery merged. Preserve that HEAD rather than rewinding its checkout;
    // `tip` remains the task's immutable delivery anchor in the receipt.
    let receiver_tip = if !foreign_source && git.ancestor(&tip, "HEAD")? {
        let head = git.run(&["rev-parse", "HEAD"])?;
        if head != tip {
            git.require_integrated(cas_root, &repo_root, task, &head, "receiver HEAD")?;
        }
        head
    } else {
        tip.clone()
    };
    if !foreign_source && !git.ancestor("HEAD", &receiver_tip)? {
        // cas-ea31 (takeover case 4): a receiver already inside the
        // integration target holds no work of its own; its task branch is
        // cut at the delivery and HEAD's work stays on the target.
        let head = git.run(&["rev-parse", "HEAD"])?;
        if git.require_integrated(cas_root, &repo_root, task, &head, "receiver HEAD").is_err() {
            return Err("receiver HEAD diverges from the task delivery; reconcile the branches first".into());
        }
    }
    let branch = worker_task_branch(&receiver.name, &task.id);
    let reference = format!("refs/heads/{branch}");
    let exists = git
        .output(&["show-ref", "--verify", "--quiet", &reference])?
        .status
        .success();
    if exists {
        if !git.ancestor(&reference, &receiver_tip)? {
            if !git.ancestor(&receiver_tip, &reference)? {
                return Err("receiver task branch diverges from the delivery ancestry; reconcile the branches first".into());
            }
            git.require_integrated(cas_root, &repo_root, task, &reference, "receiver task branch")?;
        }
        git.run(&["switch", &branch])?;
        git.run(&["merge", "--ff-only", &receiver_tip])?;
    } else {
        git.run(&["switch", "-c", &branch, &receiver_tip])?;
    }
    verify_worker_worktree_binding(&receiver.name, destination, Some(&branch))
        .map_err(|e| e.to_string())?;
    let recorded_base = task.deliverables.factory_branch_start.as_ref()
        .map(|start| start.base_sha.clone())
        .filter(|base| git.ancestor(base, &tip).unwrap_or(false));
    let base = match (recorded_base, task.deliverables.work_target.as_ref()) {
        (Some(base), _) => Some(base),
        (None, Some(target)) => {
            let reference = close_ops::preferred_diff_target_ref_for_delivery(
                &repo_root, &target.target_branch, task.delivery_mode,
            );
            git.run(&["merge-base", &tip, &reference]).ok()
        }
        (None, None) => None,
    };
    Ok(AdoptedBranch {
        source_branch,
        branch,
        tip,
        base,
    })
}
