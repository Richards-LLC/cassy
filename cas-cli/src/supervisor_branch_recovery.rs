//! Explicit recovery of a dead worker's exact committed work. Publishing uses
//! the authenticated successor's own ref and the normal private Git guard.
use std::{path::Path, process::Command, time::Duration};

use anyhow::{Context, Result, bail};
use cas_store::{AgentStore, EventStore};
use cas_types::{Agent, AgentRole, EventType};
use serde::{Deserialize, Serialize};

use crate::ui::factory::daemon::runtime::teams::TeamsManager;
use crate::{
    bounded_process::{Deadline, run_command},
    mcp::CasCore,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    branch: String,
    head: String,
    /// Empty means that the successor-owned remote ref must not exist.
    expected_remote: String,
}

#[derive(Serialize)]
pub(crate) struct Receipt {
    pub status: &'static str,
    pub task_id: String,
    pub actor: String,
    pub source_branch: String,
    pub branch: String,
    pub head: String,
    pub remote: &'static str,
}

impl Receipt {
    pub(crate) fn render(&self) -> String {
        let brief = |value: &str| -> String {
            if value.chars().count() <= 64 {
                value.into()
            } else {
                format!("{}...", value.chars().take(61).collect::<String>())
            }
        };
        format!(
            "Recovered {}\nPublished under successor authority.\nActor: {}\nSource: {}\nBranch: {}\nCommit: {}\nRemote: {}\nFull receipt: --json",
            brief(&self.task_id),
            brief(&self.actor),
            brief(&self.source_branch),
            brief(&self.branch),
            self.head,
            self.remote
        )
    }
}

fn oid(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_hexdigit())
}

struct Git<'a> {
    root: &'a Path,
    deadline: Deadline,
    actor: &'a Agent,
}
impl Git<'_> {
    fn output(&self, args: &[&str], cap: Duration) -> Result<std::process::Output> {
        run_command(
            Command::new("git")
                .arg("-C")
                .arg(self.root)
                .args(args)
                .env("GIT_OPTIONAL_LOCKS", "0")
                // This is the real authenticated actor, never the source worker.
                .env("CAS_AGENT_NAME", &self.actor.name),
            self.deadline,
            cap,
        )
        .map_err(|e| {
            anyhow::anyhow!(
                "Git {} did not complete: {e:?}",
                args.first().unwrap_or(&"operation")
            )
        })
    }
    fn run(&self, args: &[&str]) -> Result<String> {
        let result = self.output(args, Duration::from_secs(10))?;
        if !result.status.success() {
            // Git diagnostics can contain a credential-bearing remote URL.
            bail!(
                "Git {} refused (status {}); inspect repository configuration before retrying",
                args.first().unwrap_or(&"operation"),
                result.status
            );
        }
        Ok(String::from_utf8_lossy(&result.stdout).trim().to_string())
    }
    fn remote_tip(&self, url: &str, reference: &str) -> Result<String> {
        let response = self.run(&["ls-remote", "--refs", "--", url, reference])?;
        if response.is_empty() {
            return Ok(String::new());
        }
        let mut rows = response.lines();
        let row = rows.next().context("remote response missing")?;
        let (sha, observed) = row.split_once('\t').context("malformed remote response")?;
        if observed != reference || !oid(sha) || rows.next().is_some() {
            bail!("remote ref observation is ambiguous");
        }
        Ok(sha.into())
    }
}

fn ensure_dead(core: &CasCore, prior: &str) -> Result<()> {
    let agents = core
        .open_agent_store()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let rows = agents.list(None)?;
    let matching: Vec<_> = rows
        .iter()
        .filter(|a| a.name == prior || a.id == prior)
        .collect();
    for agent in &matching {
        if agent.role != AgentRole::Worker {
            bail!("source owner is not a worker");
        }
        if crate::mcp::tools::service::agent_liveness::evaluate_supervision_liveness(agent)
            .is_live()
        {
            bail!("source worker {prior} is still live; leave its checkout and lease alone");
        }
    }
    if !matching.is_empty() {
        return Ok(());
    }
    // Deleted registrations need positive retained identity, not an assumption
    // that any unregistered name authorizes a foreign branch.
    let events = cas_store::SqliteEventStore::open(&core.cas_root)?;
    let retired = events.list_by_type(EventType::AgentShutdown, 1000)?;
    let proven = retired.iter().any(|event| {
        event.metadata.as_ref().is_some_and(|m| {
            let agent = &m["retired_agent"];
            (event.entity_id == prior || agent["name"] == prior) && agent["role"] == "worker"
        })
    });
    if !proven {
        bail!("source worker retirement is unproven; retain or restore its registration evidence");
    }
    Ok(())
}

pub(crate) fn recover(core: &CasCore, task_id: &str, payload: &str) -> Result<Receipt> {
    let actor = core
        .resolve_live_supervisor_authority()
        .map_err(|_| anyhow::anyhow!("recover_branch requires a live registered supervisor"))?;
    if payload.len() > 16 * 1024 {
        bail!("recover_branch payload exceeds 16 KiB");
    }
    let request: Request = serde_json::from_str(payload).context(
        "payload must contain branch, full head SHA and expected_remote (empty for a new ref)",
    )?;
    if !oid(&request.head)
        || (!request.expected_remote.is_empty() && !oid(&request.expected_remote))
    {
        bail!("head and nonempty expected_remote must be full 40-character commit SHAs");
    }
    let tasks = core
        .open_task_store()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?;
    let task = tasks.get(task_id)?;
    if task.is_terminal() {
        bail!("cannot recover a terminal task");
    }
    let root = if let Some(target) = &task.deliverables.work_target {
        crate::mcp::tools::core::task::repo_context::resolve_repo_context(&core.cas_root, target)
            .map_err(anyhow::Error::msg)?
            .repo_root
    } else {
        core.cas_root
            .parent()
            .context("project repository missing")?
            .to_path_buf()
    };
    let deadline = Deadline::after(Duration::from_secs(120));
    let git = Git {
        root: &root,
        deadline,
        actor: &actor,
    };
    git.run(&["check-ref-format", "--branch", &request.branch])?;
    let branch = crate::factory_isolation::worker_task_branch(&actor.name, task_id);
    git.run(&["check-ref-format", "--branch", &branch])?;
    let reference = format!("refs/heads/{branch}");
    let source = format!("refs/heads/{}", request.branch);
    let actual = git.run(&[
        "rev-parse",
        "--verify",
        "--end-of-options",
        &format!("{source}^{{commit}}"),
    ])?;
    if actual != request.head {
        bail!("source branch moved; read its full SHA and submit a new explicit recovery request");
    }
    let url = git.run(&["remote", "get-url", "--push", "--all", "origin"])?;
    if url.is_empty() || url.lines().count() != 1 || url.starts_with('-') {
        bail!("origin must have exactly one push destination");
    }
    let receipt = Receipt {
        status: "recovered",
        task_id: task_id.into(),
        actor: actor.name.clone(),
        source_branch: request.branch.clone(),
        branch: branch.clone(),
        head: request.head.clone(),
        remote: "origin",
    };
    let remote_tip = git.remote_tip(&url, &reference)?;
    // An interrupted response can be safely retried after its durable task
    // update. This cannot adopt a new tip or relax the original source proof.
    if task.assignee.as_deref() == Some(actor.name.as_str())
        && task.deliverables.parked_branch.as_deref() == Some(branch.as_str())
        && task.deliverables.factory_branch_anchor.as_deref() == Some(request.head.as_str())
        && task.deliverables.handoff_branches.contains(&request.branch)
        && remote_tip == request.head
    {
        return Ok(receipt);
    }
    let prior = task
        .assignee
        .as_deref()
        .context("task has no source assignee")?;
    ensure_dead(core, prior)?;
    let expected_task = crate::factory_isolation::worker_task_branch(prior, task_id);
    let expected_worker = crate::factory_isolation::expected_worker_branch(prior);
    if request.branch != expected_task && request.branch != expected_worker {
        bail!("source branch does not belong to this task's recorded worker {prior}");
    }
    if let Some(anchor) = &task.deliverables.factory_branch_anchor {
        git.run(&["merge-base", "--is-ancestor", anchor, &request.head])?;
    }
    if remote_tip != request.expected_remote && remote_tip != request.head {
        bail!("remote ref changed; read origin's exact destination tip before retrying");
    }
    if !remote_tip.is_empty() {
        git.run(&["merge-base", "--is-ancestor", &remote_tip, &request.head])?;
    }
    // No active holder may race a dead registration's rescue.
    if let Some(lease) = core
        .open_agent_store()
        .map_err(|e| anyhow::anyhow!(e.to_string()))?
        .get_lease(task_id)?
    {
        bail!(
            "task has an active lease held by {}; release it through the normal task handoff first",
            lease.agent_id
        );
    }
    let audit = format!(
        "[{}] BRANCH RECOVERY INTENT by {} ({}): {} at {} -> {} on origin; expected remote {}",
        chrono::Utc::now().to_rfc3339(),
        actor.name,
        actor.id,
        request.branch,
        request.head,
        branch,
        if request.expected_remote.is_empty() {
            "absent"
        } else {
            &request.expected_remote
        }
    );
    tasks.append_trusted_note(task_id, &audit)?;
    let before = tasks.get(task_id)?;
    // Serialize recoveries in this repository without mutating the source's
    // checkout, hooks or HEAD. This lock is not a branch-ownership capability.
    let common = git.run(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(Path::new(&common).join("cas-branch-recovery.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .context("another branch recovery is active; retry after it finishes")?;
    ensure_dead(core, prior)?;
    if git.run(&["rev-parse", "--verify", "--end-of-options", &source])? != request.head {
        bail!("source branch moved after validation");
    }
    let exists = git.output(
        &["show-ref", "--verify", "--quiet", &reference],
        Duration::from_secs(5),
    )?;
    match exists.status.code() {
        Some(0) => {
            if git.run(&["rev-parse", "--verify", &reference])? != request.head {
                bail!("successor branch already contains different work");
            }
        }
        Some(1) => {}
        _ => bail!("cannot inspect successor branch"),
    }
    let base = core.cas_root.join("worktrees");
    std::fs::create_dir_all(&base)?;
    let worktree = tempfile::Builder::new()
        .prefix("recover-")
        .tempdir_in(&base)?;
    let path = worktree.path().to_path_buf();
    git.run(&[
        "worktree",
        "add",
        "--detach",
        path.to_str().context("non-UTF8 recovery path")?,
        &request.head,
    ])?;
    let recovered = (|| -> Result<()> {
        let destination = Git {
            root: &path,
            deadline,
            actor: &actor,
        };
        if exists.status.success() {
            destination.run(&["switch", &branch])?;
        } else {
            destination.run(&["switch", "-c", &branch])?;
        }
        TeamsManager::install_worker_pre_commit_hook(&path)?;
        // The normal private guard admits the actor's own ref. No source
        // worker environment, --no-verify or foreign-ref capability exists.
        let pushed = destination.output(
            &[
                "push",
                &format!("--force-with-lease={reference}:{remote_tip}"),
                "--",
                &url,
                &format!("{}:{reference}", request.head),
            ],
            Duration::from_secs(90),
        )?;
        if !pushed.status.success() {
            bail!(
                "recovery push failed; source work is retained and the recovery intent is recorded"
            );
        }
        if destination.remote_tip(&url, &reference)? != request.head {
            bail!("push returned but the destination tip could not be verified");
        }
        let mut next = before.clone();
        next.assignee = Some(actor.name.clone());
        next.deliverables.record_handoff_branch(&request.branch);
        next.deliverables.parked_branch = Some(branch.clone());
        next.deliverables.factory_branch_anchor = Some(request.head.clone());
        next.notes.push_str(&format!(
            "\n\n[{}] BRANCH RECOVERED by {} ({}): {} at {} -> {} on origin",
            chrono::Utc::now().to_rfc3339(),
            actor.name,
            actor.id,
            request.branch,
            request.head,
            branch
        ));
        tasks.update_if_current(&next, &before).context("push completed; task changed concurrently, so reload its recorded recovery intent before rebinding")?;
        Ok(())
    })();
    let cleanup = git.run(&[
        "worktree",
        "remove",
        path.to_str().context("non-UTF8 recovery path")?,
    ]);
    if cleanup.is_err() {
        let retained = worktree.keep();
        tasks.append_trusted_note(
            task_id,
            &format!(
                "Recovery worktree retained at {} after cleanup refusal",
                retained.display()
            ),
        )?;
    }
    if let Err(error) = &recovered {
        tasks.append_trusted_note(
            task_id,
            &format!("BRANCH RECOVERY INCOMPLETE by {}: {error}", actor.id),
        )?;
    }
    recovered?;
    Ok(receipt)
}
