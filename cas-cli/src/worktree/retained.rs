//! Read-only inventory of retired worker checkouts, including those absent
//! from the active worktree store. Shared by startup and factory diagnostics.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use cas_types::{AgentRole, Task};

use super::GitOperations;

#[derive(Debug)]
pub(crate) struct RetainedWorktree {
    pub worker: String,
    pub task: Option<String>,
    pub path: PathBuf,
    pub branch: Option<String>,
    pub push_remote: Option<String>,
    pub head: String,
    pub unpushed: Option<usize>,
    pub dirty: Option<usize>,
    errors: Vec<String>,
}

#[derive(Debug, Default)]
pub(crate) struct RetainedInventory {
    pub entries: Vec<RetainedWorktree>,
    pub errors: Vec<String>,
}

impl RetainedWorktree {
    pub fn summary(&self) -> String {
        format!(
            "{} task={} HEAD={} unpushed={} dirty={} path={}",
            self.worker,
            self.task.as_deref().unwrap_or("unknown"),
            self.head,
            count(self.unpushed),
            count(self.dirty),
            self.path.display(),
        )
    }

    pub fn matches_task(&self, task: &Task) -> bool {
        self.task.as_deref() == Some(task.id.as_str())
            || self
                .branch
                .as_deref()
                .is_some_and(|branch| task_has_branch(task, branch))
    }
}

impl RetainedInventory {
    pub fn is_partial(&self) -> bool {
        !self.errors.is_empty() || self.entries.iter().any(|entry| !entry.errors.is_empty())
    }

    pub fn needs_attention(&self) -> bool {
        !self.entries.is_empty() || !self.errors.is_empty()
    }

    pub fn compact(&self) -> String {
        format!(
            "Retained worker worktrees with local work: {}{}. Review `cas factory gc report --full`; commit dirty files after review and recover with `factory action=recover_branch task_id=<task> payload=<expected tips>`.\n",
            self.entries.len(),
            if !self.is_partial() {
                ""
            } else {
                " (partial inventory; inspect errors)"
            },
        )
    }

    pub fn render(&self) -> String {
        if !self.needs_attention() {
            return String::new();
        }
        let mut out = format!(
            "\nRetained worker worktrees with local work: {}\n",
            self.entries.len()
        );
        for entry in &self.entries {
            out.push_str(&format!("  - {}\n", entry.summary()));
            for error in &entry.errors {
                out.push_str(&format!("    Inventory incomplete: {error}\n"));
            }
            if let (Some(branch), Some(remote)) = (&entry.branch, &entry.push_remote) {
                out.push_str(&format!(
                    "    Recovery (owner): review/commit dirty files, then git -C {} push {} {}\n",
                    shell_quote(&entry.path.to_string_lossy()),
                    shell_quote(remote),
                    shell_quote(&format!("HEAD:refs/heads/{branch}")),
                ));
            }
        }
        out.push_str("  Successor supervisor: use factory action=recover_branch with task_id and expected branch/HEAD/remote tips; review dirty files first. No recovery or cleanup was performed.\n");
        for error in &self.errors {
            out.push_str(&format!("  Inventory incomplete: {error}\n"));
        }
        out
    }
}

fn count(value: Option<usize>) -> String {
    value.map_or_else(|| "unknown".into(), |n| n.to_string())
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn task_has_branch(task: &Task, branch: &str) -> bool {
    task.deliverables.parked_branch.as_deref() == Some(branch)
        || task
            .deliverables
            .factory_branch_start
            .as_ref()
            .is_some_and(|start| start.branch == branch)
        || task
            .deliverables
            .handoff_branches
            .iter()
            .any(|b| b == branch)
}

/// No network, ref mutation or index refresh. A shared deadline bounds the
/// whole inventory; unknown counts and skipped probes remain visible.
pub(crate) fn scan(cas_root: &Path) -> RetainedInventory {
    scan_until(cas_root, Instant::now() + Duration::from_secs(2))
}

pub(crate) fn scan_until(cas_root: &Path, deadline: Instant) -> RetainedInventory {
    let mut report = RetainedInventory::default();
    let repo = cas_root.parent().unwrap_or(cas_root);
    // A not-yet-initialized project has no Git worktrees to inspect.
    if !repo.join(".git").exists() {
        return report;
    }
    let agents = crate::store::open_agent_store(cas_root).and_then(|store| Ok(store.list(None)?));
    let agents = match agents {
        Ok(agents) => agents,
        Err(error) => {
            report
                .errors
                .push(format!("worker identities unavailable: {error}"));
            Vec::new()
        }
    };
    let tasks = crate::store::open_task_store(cas_root).and_then(|store| Ok(store.list(None)?));
    let tasks = match tasks {
        Ok(tasks) => tasks,
        Err(error) => {
            report
                .errors
                .push(format!("task identities unavailable: {error}"));
            Vec::new()
        }
    };
    let listing = match git(repo, &["worktree", "list", "--porcelain", "-z"], deadline) {
        Ok(listing) => listing,
        Err(error) => {
            report.errors.push(error);
            return report;
        }
    };
    let remotes = match git(repo, &["remote"], deadline) {
        Ok(output) => output.lines().map(str::to_string).collect::<Vec<_>>(),
        Err(error) => {
            report.errors.push(error);
            return report;
        }
    };
    let push_remote = remotes
        .iter()
        .find(|remote| remote.as_str() == "origin")
        .or_else(|| remotes.first())
        .cloned();
    // Git lists the primary checkout first. Its branch is the local fallback
    // when no task target or configured base exists; never count all history.
    let local_target = crate::config::Config::configured_epic_base_branch(repo).or_else(|| {
        listing
            .split("\0\0")
            .next()?
            .split('\0')
            .find_map(|field| field.strip_prefix("branch refs/heads/").map(str::to_string))
    });
    // With -z, even spaces, quotes and newlines in checkout paths are literal.
    for record in listing.split("\0\0").filter(|record| !record.is_empty()) {
        let fields: Vec<_> = record.split('\0').collect();
        let Some(path) = fields
            .iter()
            .find_map(|field| field.strip_prefix("worktree "))
            .map(PathBuf::from)
        else {
            continue;
        };
        let branch = fields
            .iter()
            .find_map(|field| field.strip_prefix("branch refs/heads/"))
            .map(str::to_string);
        let head = fields
            .iter()
            .find_map(|field| field.strip_prefix("HEAD "))
            .unwrap_or("unknown")
            .to_string();
        let owners: Vec<_> = agents
            .iter()
            .filter(|agent| {
                agent.role == AgentRole::Worker
                    && (agent.metadata.get("clone_path").map_or_else(
                        || {
                            branch.as_deref().is_some_and(|b| {
                                b == format!("factory/{}", agent.name)
                                    || b.starts_with(&format!("factory/{}-cas-", agent.name))
                            })
                        },
                        |p| Path::new(p) == path,
                    ))
            })
            .collect();
        if !branch.as_deref().is_some_and(|b| b.starts_with("factory/")) && owners.is_empty() {
            continue;
        }
        if owners.iter().any(|agent| {
            crate::mcp::tools::service::agent_liveness::evaluate_supervision_liveness(agent)
                .is_live()
        }) {
            continue;
        }
        if Instant::now() >= deadline {
            report
                .errors
                .push("deadline reached; remaining worker checkouts were not inspected".into());
            break;
        }
        let worker = owners
            .first()
            .map(|agent| agent.name.clone())
            .unwrap_or_else(|| {
                branch
                    .as_deref()
                    .and_then(|b| b.strip_prefix("factory/"))
                    .unwrap_or("unknown")
                    .split("-cas-")
                    .next()
                    .unwrap_or("unknown")
                    .to_string()
            });
        let task = tasks
            .iter()
            .filter(|task| branch.as_deref().is_some_and(|b| task_has_branch(task, b)))
            .max_by_key(|task| task.updated_at)
            .or_else(|| {
                tasks
                    .iter()
                    .filter(|task| {
                        branch
                            .as_deref()
                            .is_some_and(|b| b.ends_with(&format!("-{}", task.id)))
                    })
                    .max_by_key(|task| task.updated_at)
            })
            .or_else(|| {
                tasks
                    .iter()
                    .filter(|task| {
                        task.assignee.as_deref().is_some_and(|a| {
                            a == worker || owners.iter().any(|owner| owner.id == a)
                        })
                    })
                    .max_by_key(|task| task.updated_at)
            });
        let target = task
            .and_then(|task| task.deliverables.work_target.as_ref())
            .map(|target| target.target_branch.as_str())
            .or(local_target.as_deref());
        let mut entry = RetainedWorktree {
            worker,
            task: task.map(|task| task.id.clone()),
            path,
            branch,
            push_remote: push_remote.clone(),
            head,
            unpushed: None,
            dirty: None,
            errors: Vec::new(),
        };
        let unpushed = if remotes.is_empty() {
            target
                .ok_or_else(|| "no local target branch available".to_string())
                .and_then(|target| {
                    git(
                        &entry.path,
                        &["rev-list", "--count", "HEAD", "--not", target],
                        deadline,
                    )
                })
        } else {
            git(
                &entry.path,
                &["rev-list", "--count", "HEAD", "--not", "--remotes"],
                deadline,
            )
        };
        match unpushed.and_then(|output| {
            output
                .trim()
                .parse::<usize>()
                .map_err(|error| error.to_string())
        }) {
            Ok(n) => entry.unpushed = Some(n),
            Err(error) => entry.errors.push(format!("unpushed count: {error}")),
        }
        match git(
            &entry.path,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            deadline,
        ) {
            Ok(output) => {
                let mut fields = output.split('\0').filter(|field| !field.is_empty());
                let mut n = 0;
                while let Some(field) = fields.next() {
                    n += 1;
                    if field
                        .as_bytes()
                        .get(..2)
                        .is_some_and(|xy| xy.contains(&b'R') || xy.contains(&b'C'))
                    {
                        fields.next(); // renamed/copied source path is not another file
                    }
                }
                entry.dirty = Some(n);
            }
            Err(error) => entry.errors.push(format!("dirty count: {error}")),
        }
        if entry.unpushed != Some(0) || entry.dirty != Some(0) {
            report.entries.push(entry);
        }
    }
    report
}

fn git(repo: &Path, args: &[&str], deadline: Instant) -> Result<String, String> {
    let remaining = deadline
        .checked_duration_since(Instant::now())
        .ok_or("inventory deadline reached")?;
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(repo)
        .args(args)
        .env("GIT_OPTIONAL_LOCKS", "0");
    let output =
        GitOperations::run_command_bounded(command, remaining.min(Duration::from_millis(500)))
            .map_err(|error| format!("git {}: {error}", args[0]))?;
    if !output.status.success() {
        return Err(format!(
            "git {}: {}",
            args[0],
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    String::from_utf8(output.stdout).map_err(|error| format!("git {}: {error}", args[0]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cas_types::{Agent, AgentStatus, TaskStatus, WorkTarget};

    fn commit_git(repo: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "CAS Test")
            .env("GIT_AUTHOR_EMAIL", "cas@example.test")
            .env("GIT_COMMITTER_NAME", "CAS Test")
            .env("GIT_COMMITTER_EMAIL", "cas@example.test")
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_SYSTEM", "/dev/null")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_string()
    }

    #[test]
    fn local_only_primary_is_not_retained_but_worker_work_is_cas_6a12() {
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        for remote in [None, Some("upstream")] {
            let dir = tempfile::tempdir().unwrap();
            let repo = dir.path();
            let root = repo.join(".cas");
            std::fs::create_dir(&root).unwrap();
            env.set("CAS_ROOT", &root);
            commit_git(repo, &["init", "-qb", "main"]);
            std::fs::write(repo.join(".gitignore"), ".cas/\n").unwrap();
            commit_git(repo, &["add", ".gitignore"]);
            commit_git(repo, &["commit", "-qm", "seed"]);
            commit_git(
                repo,
                &["commit", "--allow-empty", "-qm", "second local commit"],
            );
            let bare = tempfile::tempdir().unwrap();
            if let Some(remote) = remote {
                commit_git(bare.path(), &["init", "-q", "--bare"]);
                commit_git(
                    repo,
                    &["remote", "add", remote, bare.path().to_str().unwrap()],
                );
                commit_git(repo, &["push", "-q", remote, "main"]);
            }
            let agents = crate::store::open_agent_store(&root).unwrap();
            agents.init().unwrap();
            let tasks = crate::store::open_task_store(&root).unwrap();
            tasks.init().unwrap();
            let register = |worker: &str, path: &Path, branch: &str| {
                let mut agent =
                    Agent::new_with_role(worker.into(), worker.into(), AgentRole::Worker);
                agent.status = AgentStatus::Shutdown;
                agent
                    .metadata
                    .insert("clone_path".into(), path.display().to_string());
                agents.register(&agent).unwrap();
                let mut task = Task::new(format!("cas-{worker}"), worker.into());
                task.status = TaskStatus::Closed;
                task.assignee = Some(worker.into());
                task.deliverables.parked_branch = Some(branch.into());
                task.deliverables.work_target = Some(WorkTarget {
                    repo_selector: "project:retained-local-fixture".into(),
                    target_branch: "main".into(),
                });
                tasks.add(&task).unwrap();
            };
            register("retired-primary", repo, "main");
            let scan_now = || scan_until(&root, Instant::now() + Duration::from_secs(20));
            let clean = scan_now();
            assert!(!clean.is_partial(), "{clean:?}");
            assert!(clean.entries.is_empty(), "{clean:?}");
            let worker = root.join("retained-worker");
            commit_git(
                repo,
                &[
                    "worktree",
                    "add",
                    "-qb",
                    "factory/retired-worker",
                    worker.to_str().unwrap(),
                    "main",
                ],
            );
            register("retired-worker", &worker, "factory/retired-worker");
            std::fs::write(worker.join("local.txt"), "retained work\n").unwrap();
            commit_git(&worker, &["add", "local.txt"]);
            commit_git(&worker, &["commit", "-qm", "worker ahead"]);
            std::fs::write(worker.join("dirty.txt"), "uncommitted\n").unwrap();
            let retained = scan_now();
            assert!(!retained.is_partial(), "{retained:?}");
            assert_eq!(retained.entries.len(), 1, "{retained:?}");
            let entry = &retained.entries[0];
            assert_eq!(entry.path, worker);
            assert_eq!(entry.unpushed, Some(1));
            assert_eq!(entry.dirty, Some(1));
            assert_eq!(entry.push_remote.as_deref(), remote);
            // Dirty-only work remains visible after its commits reach target.
            commit_git(&worker, &["reset", "--hard", "main"]);
            let dirty_only = scan_now();
            assert_eq!(dirty_only.entries.len(), 1, "{dirty_only:?}");
            assert_eq!(dirty_only.entries[0].unpushed, Some(0));
            assert_eq!(dirty_only.entries[0].dirty, Some(1));
        }
    }
}
