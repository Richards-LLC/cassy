//! Durable Commander decision for a failed hotfix walk. Delivery of an ask is
//! not a decision: only a bound, hub-verified reply or the deadline selects it.
use anyhow::{Context, Result, bail};
use cas_types::{AgentRole, Task, TaskStatus};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

const WAIT_MINUTES: i64 = 15;

#[derive(Serialize, Deserialize)]
struct Decision {
    task: String,
    anchor: String,
    merge: String,
    assignee: Option<String>,
    work_target: cas_types::WorkTarget,
    repo: PathBuf,
    target: String,
    session: String,
    owner: String,
    row: String,
    ask: i64,
    deadline: DateTime<Utc>,
    choice: Option<String>,
    authority: Option<String>,
    revert_url: Option<String>,
    done: bool,
}

fn directory(root: &Path) -> PathBuf {
    root.join("fast-lane-walk")
}

fn record_path(root: &Path, task: &Task) -> Option<PathBuf> {
    let merge = task.deliverables.green_merge_commit.as_deref()?;
    let key = serde_json::to_vec(&(&task.id, merge)).ok()?;
    Some(directory(root).join(format!("{:x}.json", Sha256::digest(key))))
}

fn lock(path: &Path) -> Result<File> {
    fs::create_dir_all(path.parent().context("decision directory")?)?;
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))?;
    fs2::FileExt::try_lock_exclusive(&file)?;
    Ok(file)
}

fn save(path: &Path, decision: &Decision) -> Result<()> {
    let mut temp = tempfile::NamedTempFile::new_in(path.parent().context("decision directory")?)?;
    serde_json::to_writer_pretty(&mut temp, decision)?;
    temp.as_file().sync_all()?;
    temp.persist(path)?;
    File::open(path.parent().context("decision directory")?)?.sync_all()?;
    Ok(())
}

fn waiting(decision: &Decision) -> String {
    format!(
        "FAST LANE QA WALK FAILED on merge {}: {}. Commander ask {} offers Revert or Fix-forward; no answer within {WAIT_MINUTES} minutes defaults to Revert (deadline {}). The task waits for this decision.",
        decision.merge,
        decision.row,
        decision.ask,
        decision.deadline.to_rfc3339()
    )
}

/// Even if the ledger is edited after asking, the delivery still owes the
/// decision already requested. A repeated close never resets its deadline.
pub(crate) fn pending(root: &Path, task: &Task) -> Result<Option<String>> {
    let Some(path) = record_path(root, task).filter(|path| path.exists()) else {
        return Ok(None);
    };
    let decision: Decision = serde_json::from_slice(&fs::read(path)?)?;
    Ok((!decision.done).then(|| waiting(&decision)))
}

/// Called only from the trusted, merged fast-lane receipt gate. Reuses the
/// existing Commander operator-turn outbox; no second channel or model hop.
pub(crate) fn ask(
    root: &Path,
    task: &Task,
    repo: &Path,
    target: &str,
    owner: &str,
    row: &str,
) -> Result<String> {
    let path = record_path(root, task).context("merged delivery identity is missing")?;
    let _lock = lock(&path)?;
    if path.exists() {
        let decision: Decision = serde_json::from_slice(&fs::read(&path)?)?;
        return Ok(waiting(&decision));
    }
    let agent = crate::store::open_agent_store(root)?.get(owner)?;
    if agent.role != AgentRole::Supervisor {
        bail!("the QA decision needs a registered factory supervisor");
    }
    let session = agent
        .factory_session
        .clone()
        .context("supervisor has no factory session")?;
    let merge = task
        .deliverables
        .green_merge_commit
        .clone()
        .context("missing merge")?;
    let anchor = task
        .deliverables
        .factory_branch_anchor
        .clone()
        .context("missing delivery anchor")?;
    let work_target = task
        .deliverables
        .work_target
        .clone()
        .context("missing work target")?;
    let now = Utc::now();
    let message = format!(
        "The production QA walk failed for {} on merge {merge}: {row}. Revert opens a revert PR and returns the task to its worker; Fix-forward returns it without a revert PR. No answer within {WAIT_MINUTES} minutes defaults to Revert.",
        task.id
    );
    let summary = format!("Failed hotfix QA walk: {}", task.id);
    let payload = crate::ui::factory::OperatorReplyPayload {
        schema_version: 2,
        reply_to: None,
        message,
        summary: summary.clone(),
        device_id: "*".into(),
        operator_label: None,
        kind: crate::ui::factory::OperatorTurnKind::Ask,
        options: vec!["Revert".into(), "Fix-forward".into()],
        attachments: Vec::new(),
        reply_to_session: None,
    };
    let queue = crate::store::open_prompt_queue_store(root)?;
    let key = format!("fast-lane-walk:{}:{merge}", task.id);
    let payload = serde_json::to_string(&payload)?;
    let ask = queue
        .record_operator_turn(&cas_store::OperatorTurn {
            source: "supervisor",
            target: "operator",
            prompt: &payload,
            factory_session: Some(&session),
            metadata: cas_store::OperatorTurnMetadata {
                summary: Some(&summary),
                origin: Some(&cas_store::QueueOrigin::Daemon),
                kind: Some("ask"),
                dedupe_key: Some(&key),
                occurred_at: Some(now),
                ..Default::default()
            },
        })?
        .id();
    // The deduplicated ask's original creation time survives a crash before
    // this file is persisted. Retries cannot buy another fifteen minutes.
    let created = queue
        .queued_prompt(ask)?
        .context("ask disappeared")?
        .created_at;
    let decision = Decision {
        task: task.id.clone(),
        anchor,
        merge,
        assignee: task.assignee.clone(),
        work_target,
        repo: repo.canonicalize()?,
        target: target.strip_prefix("origin/").unwrap_or(target).into(),
        session,
        owner: owner.into(),
        row: row.into(),
        ask,
        deadline: created + Duration::minutes(WAIT_MINUTES),
        choice: None,
        authority: None,
        revert_url: None,
        done: false,
    };
    save(&path, &decision)?;
    Ok(waiting(&decision))
}

fn advance(root: &Path, path: &Path, now: DateTime<Utc>) -> Result<()> {
    let _lock = lock(path)?;
    let mut decision: Decision = serde_json::from_slice(&fs::read(path)?)?;
    if decision.done {
        return Ok(());
    }
    let tasks = crate::store::open_task_store(root)?;
    let task = tasks.get(&decision.task)?;
    let queue = crate::store::open_prompt_queue_store(root)?;
    if task.status != TaskStatus::AwaitingMerge
        || task.assignee != decision.assignee
        || task.deliverables.factory_branch_anchor.as_deref() != Some(&decision.anchor)
        || task.deliverables.green_merge_commit.as_deref() != Some(&decision.merge)
        || task.deliverables.work_target.as_ref() != Some(&decision.work_target)
        || crate::fast_lane::merged_delivery(&task).is_none()
    {
        queue.mark_superseded(decision.ask, "QA decision withdrawn: delivery changed")?;
        decision.done = true;
        return save(path, &decision);
    }
    // Resolve the pinned repository again before any external Git mutation.
    let context = crate::mcp::tools::core::task::repo_context::resolve_repo_context(
        root,
        &decision.work_target,
    )
    .map_err(anyhow::Error::msg)?;
    if context.repo_root.canonicalize()? != decision.repo
        || context
            .target_branch
            .strip_prefix("origin/")
            .unwrap_or(&context.target_branch)
            != decision.target
    {
        bail!("QA decision repository or target changed");
    }
    if decision.choice.is_none() {
        let selected = queue
            .verified_operator_replies_to(decision.ask)?
            .into_iter()
            .find(|reply| {
                reply.created_at <= decision.deadline
                    && reply.created_at <= now
                    && matches!(reply.prompt.as_str(), "Revert" | "Fix-forward")
            });
        if let Some(reply) = selected {
            decision.choice = Some(reply.prompt);
            decision.authority = Some(format!(
                "verified Commander reply {} to ask {}",
                reply.id, decision.ask
            ));
        } else if now >= decision.deadline {
            decision.choice = Some("Revert".into());
            decision.authority = Some(format!(
                "no valid answer to ask {} by {}; fifteen-minute default",
                decision.ask, decision.deadline
            ));
        } else {
            return Ok(());
        }
        // Persist the winner before external work. Late replies cannot change
        // an action whose PR failed temporarily or whose process restarted.
        save(path, &decision)?;
    }
    let authority = decision
        .authority
        .as_deref()
        .context("decision authority missing")?;
    let mut reason = format!(
        "FAST LANE QA WALK FAILED on merge {}: {}. {} selected {}.",
        decision.merge,
        decision.row,
        authority,
        decision.choice.as_deref().unwrap_or_default()
    );
    if decision.choice.as_deref() == Some("Revert") {
        if decision.revert_url.is_none() {
            let failed = crate::fast_lane::Check {
                name: "Production QA walk".into(),
                state: crate::fast_lane::State::Failure,
                url: None,
                run_id: None,
            };
            let url = match crate::fast_lane::reverted(&task.notes, &decision.merge) {
                Some(url) => url,
                None => crate::fast_lane::open_revert_pr(
                    &decision.repo,
                    &decision.target,
                    &decision.merge,
                    &task.id,
                    &failed,
                    &reason,
                    &crate::github_repo::gh_binary(),
                )
                .map_err(anyhow::Error::msg)?,
            };
            decision.revert_url = Some(url);
            save(path, &decision)?;
        }
        let url = decision
            .revert_url
            .as_deref()
            .context("revert URL missing")?;
        tasks.append_trusted_note(
            &task.id,
            &crate::fast_lane::revert_note(&decision.merge, "Production QA walk", url),
        )?;
        reason.push_str(&format!(
            " Revert PR: {url}. Merge it once its merge checks pass and tell the operator."
        ));
    }
    let detail = crate::mcp::tools::core::task::lifecycle::merge_close::return_merged_delivery_for_source_fix(
        root, &decision.owner, &task, &decision.repo, &decision.target, &reason,
    ).map_err(anyhow::Error::msg)?;
    if detail.is_empty() || tasks.get(&task.id)?.status != TaskStatus::Open {
        bail!("QA decision could not return the current delivery: {detail}");
    }
    // Suppression retires an expired/answered ask; it does not assert that an
    // operator saw it or acted. The authority above is the durable action proof.
    queue.mark_superseded(decision.ask, &reason)?;
    decision.done = true;
    save(path, &decision)
}

/// Same deterministic step in production and tests; the clock is injected.
pub(crate) fn tick(root: &Path, now: DateTime<Utc>) -> Result<()> {
    let directory = directory(root);
    if !directory.exists() {
        return Ok(());
    }
    let mut failure = None;
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        if let Err(error) = advance(root, &path, now) {
            if !crate::qa_post_release::lock_contention(&error) {
                failure = Some(error);
            }
        }
    }
    // One broken delivery must not prevent another delivery's deadline action.
    match failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

/// One off-loop runner per project, including pending decisions on restart.
pub(crate) fn spawn_runner(root: PathBuf) {
    if crate::fast_lane::configured(&root).is_none() && !directory(&root).exists() {
        return;
    }
    let lease = match lock(&directory(&root).join("runner")) {
        Ok(lease) => lease,
        Err(error) => {
            if !crate::qa_post_release::lock_contention(&error) {
                tracing::warn!(%error, "fast-lane QA runner lease failed");
            }
            return;
        }
    };
    tokio::spawn(async move {
        let _lease = lease;
        loop {
            let root = root.clone();
            match tokio::task::spawn_blocking(move || tick(&root, Utc::now())).await {
                Ok(Ok(())) => {}
                result => tracing::warn!(?result, "fast-lane QA decision tick failed"),
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}
