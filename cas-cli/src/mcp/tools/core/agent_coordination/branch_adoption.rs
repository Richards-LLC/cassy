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
            .and_then(|store| store.get_parent_epic(&task.id))
            .map_err(|error| format!("cannot resolve task integration target: {error}"))?;
        let target = match close_ops::effective_close_work_target(task, parent.as_ref()) {
            Some(target) => target.target_branch,
            None => {
                let worktree_parent = task.worktree_id.as_deref().map(|id| {
                    open_worktree_store(cas_root)
                        .and_then(|store| store.get(id))
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
    let prior = task
        .assignee
        .as_deref()
        .ok_or("task has no prior assignee")?;
    let mut candidates = vec![worker_task_branch(prior, &task.id)];
    candidates.extend(task.deliverables.parked_branch.iter().cloned());
    candidates.extend(task.deliverables.handoff_branches.iter().rev().cloned());
    candidates.push(crate::factory_isolation::expected_worker_branch(prior));
    let mut source = None;
    for branch in candidates {
        if !branch.starts_with("factory/") {
            continue;
        }
        let reference = format!("refs/heads/{branch}^{{commit}}");
        let output = git.output(&["rev-parse", "--verify", "--end-of-options", &reference])?;
        if output.status.success() {
            source = Some((
                branch,
                String::from_utf8_lossy(&output.stdout).trim().to_string(),
            ));
            break;
        }
    }
    let (source_branch, source_head) =
        source.ok_or("no local factory delivery branch found for this task")?;
    let tip = if let Some(anchor) = &task.deliverables.factory_branch_anchor {
        let reference = format!("{anchor}^{{commit}}");
        let tip = git.run(&["rev-parse", "--verify", "--end-of-options", &reference])?;
        if !git.ancestor(&tip, &source_head)? {
            return Err("task delivery anchor is not on the source branch".into());
        }
        tip
    } else {
        source_head
    };
    let current_branch = git.run(&["symbolic-ref", "--short", "HEAD"])?;
    // cas-a9fe: a supervisor may repair a receiver already checked out on
    // this task's source branch. Prove the registered path is a real root
    // and accept only that exact source, never an unrelated foreign branch.
    let foreign_source = current_branch == source_branch
        && classify_worker_binding(&receiver.name, Some(&current_branch)) != WorkerBinding::Own;
    verify_worker_worktree_binding(
        &receiver.name,
        destination,
        foreign_source.then_some(source_branch.as_str()),
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
        return Err("receiver HEAD diverges from the task delivery; reconcile the branches first".into());
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
    Ok(AdoptedBranch {
        source_branch,
        branch,
        tip,
    })
}
