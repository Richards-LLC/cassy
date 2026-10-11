//! Opt-in fast-forward handoff into a receiving worker's own checkout.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::Duration;

use cas_types::{Agent, AgentRole, Task};

use crate::bounded_process::{Deadline, run_command};
use crate::factory_isolation::{verify_worker_worktree_binding, worker_task_branch};

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
}

pub(super) fn adopt_task_branch(
    cas_root: &Path,
    task: &Task,
    receiver: &Agent,
) -> Result<AdoptedBranch, String> {
    if task.is_terminal() || receiver.role != AgentRole::Worker {
        return Err("only a nonterminal task can adopt a registered worker's branch".into());
    }
    let destination = receiver
        .metadata
        .get("clone_path")
        .filter(|path| !path.trim().is_empty())
        .ok_or("receiver has no registered clone_path; spawn an isolated worker first")?;
    let destination = Path::new(destination);
    verify_worker_worktree_binding(&receiver.name, destination, None).map_err(|e| e.to_string())?;
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
    if !git.ancestor("HEAD", &tip)? {
        return Err("receiver HEAD cannot fast-forward to the task delivery tip; reconcile the branches first".into());
    }
    let branch = worker_task_branch(&receiver.name, &task.id);
    let reference = format!("refs/heads/{branch}");
    let exists = git
        .output(&["show-ref", "--verify", "--quiet", &reference])?
        .status
        .success();
    if exists {
        if !git.ancestor(&reference, &tip)? {
            return Err("receiver task branch cannot fast-forward to the delivery tip".into());
        }
        git.run(&["switch", &branch])?;
        git.run(&["merge", "--ff-only", &tip])?;
    } else {
        git.run(&["switch", "-c", &branch, &tip])?;
    }
    Ok(AdoptedBranch {
        source_branch,
        branch,
        tip,
    })
}
