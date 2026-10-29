use crate::ui::factory::daemon::imports::*;

/// The daemon's refresh tick: reminders and GitHub watcher results are
/// handled at this cadence.
pub(super) const DAEMON_REFRESH_INTERVAL: Duration = Duration::from_secs(2);
/// How often the loop runs while an urgent inject is settling (cas-786a6).
const URGENT_INJECT_POLL_MS: u64 = 25;

/// Stable identity for one alarm episode, independent of delivery retries.
/// Session scope belongs to the caller's outer key. Tuple encoding prevents
/// separators inside a kind or subject from aliasing a different episode.
pub(crate) fn attention_episode_key(
    kind: &str,
    subject: &str,
    episode_start: chrono::DateTime<chrono::Utc>,
) -> String {
    serde_json::json!([kind, subject, episode_start.to_rfc3339()]).to_string()
}

/// Route only typed routine nudges through the durable lane. The fallback
/// baseline is durable task state (or a fixed unknown baseline), never retry time.
/// With no baseline, UNIX_EPOCH coalesces unknown episodes for this worker/task/session
/// until a durable baseline becomes available.
fn enqueue_stall_nudge_prompt(
    cas_dir: &std::path::Path,
    session: &str,
    prompt: &crate::ui::factory::director::Prompt,
    episode_start: Option<chrono::DateTime<chrono::Utc>>,
) -> anyhow::Result<Option<i64>> {
    let Some(task_id) = prompt.stall_nudge_task.as_deref() else { return Ok(None); };
    let start = episode_start.unwrap_or_else(|| {
        let fallback = crate::store::open_task_store_local(cas_dir).ok()
            .and_then(|store| store.get(task_id).ok()).map(|task| task.updated_at)
            .unwrap_or(chrono::DateTime::UNIX_EPOCH);
        tracing::warn!(worker = %prompt.target, task_id, %fallback,
            "stall nudge missing episode baseline; using durable fallback");
        fallback
    });
    super::delivery::enqueue_worker_stall_nudge(cas_dir, session, &prompt.target, task_id,
        &prompt.text, start).map(Some)
}

/// Persist one worker-health incident and hand it to the supervisor's durable
/// prompt queue. The detector has already debounced the episode and the event
/// has already been delivery-revalidated when this runs.
#[cfg(test)]
fn enqueue_worker_attention_relay(
    cas_dir: &std::path::Path,
    event: &crate::ui::factory::director::DirectorEvent,
) -> WorkerAttentionRelayOutcome {
    enqueue_worker_attention_relay_for_episode(cas_dir, event, Some(chrono::DateTime::UNIX_EPOCH))
}

/// cas-7456 (GH #1302 D): the supervisor has left its idle page unanswered
/// past [`SUPERVISOR_IDLE_OPERATOR_PAGE_SECS`] while work waits, so the
/// operator hears it too. The tracker asks on every such wake; this records
/// one durable operator turn per silence `episode` (the supervisor's last
/// activity), so the stored row is the "already paged" mark (cas-b080).
///
/// [`SUPERVISOR_IDLE_OPERATOR_PAGE_SECS`]: crate::ui::factory::director::SUPERVISOR_IDLE_OPERATOR_PAGE_SECS
fn page_operator_for_idle_supervisor(
    cas_dir: &std::path::Path,
    next_step: &crate::ui::factory::director::SupervisorActionableState,
    episode: chrono::DateTime<chrono::Utc>,
) {
    let session = std::env::var("CAS_FACTORY_SESSION").unwrap_or_default();
    let supervisor = crate::store::open_agent_store(cas_dir)
        .map(|agents| {
            crate::mcp::tools::core::task::lifecycle::supervisor_push::resolve_notice_recipient(
                agents.as_ref(),
                (!session.is_empty()).then_some(session.as_str()),
            )
            .name
        })
        .unwrap_or_else(|_| "the supervisor".to_string());
    let hours = crate::ui::factory::director::SUPERVISOR_IDLE_OPERATOR_PAGE_SECS / 3600;
    let summary = format!("Supervisor {supervisor} idle {hours}h+ with work waiting");
    let message = format!(
        "Supervisor {supervisor} has taken no action for over {hours} hours and has not answered its idle pages, while work waits:\n{}\nType in its pane to wake it, or take the work over.",
        next_step.next_step_text()
    );
    let Ok(queue) = crate::store::open_prompt_queue_store(cas_dir) else {
        tracing::warn!("cas-7456: prompt queue unavailable; supervisor-idle operator page skipped");
        return;
    };
    let payload = match serde_json::to_string(&crate::ui::factory::OperatorReplyPayload {
        schema_version: 2,
        reply_to: None,
        message,
        summary: summary.clone(),
        device_id: "*".to_string(),
        operator_label: None,
        kind: crate::ui::factory::OperatorTurnKind::Blocker,
        options: Vec::new(),
        attachments: Vec::new(),
        reply_to_session: None,
    }) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::warn!(%error, "cas-7456: failed to encode the supervisor-idle operator page");
            return;
        }
    };
    if let Err(error) = queue.record_operator_turn(&cas_store::OperatorTurn {
        source: "supervisor-idle-watchdog",
        target: "operator",
        prompt: &payload,
        factory_session: (!session.is_empty()).then_some(session.as_str()),
        metadata: cas_store::OperatorTurnMetadata {
            summary: Some(summary.as_str()),
            priority: Some(cas_store::NotificationPriority::High),
            origin: Some(&cas_store::QueueOrigin::Daemon),
            kind: Some("blocker"),
            dedupe_key: Some(&format!("supervisor-idle-page:{session}:{}", episode.to_rfc3339())),
            ..Default::default()
        },
    }) {
        tracing::warn!(%error, "cas-7456: failed to record the supervisor-idle operator page");
    }
}

/// How far back an expired verification dispatch is still escalated, so an
/// upgrade does not page for old history.
const EXPIRED_DISPATCH_ESCALATION_WINDOW_HOURS: i64 = 24;

/// cas-7456 (GH #1302 B): a verification dispatch whose deadline passed with
/// no verdict wakes the supervisor through a durable worker-attention relay,
/// one per dispatch (the relay is keyed on the dispatch id, so each sweep's
/// repeat is idempotent). Its lifecycle-wake source puts it under the GH #894
/// watchdog, which tells the operator if the supervisor never receives it.
///
/// cas-b080: dispatches already woken (their relay's prompt is delivered) are
/// skipped before the per-sweep cap, so a burst of expiries cannot keep the
/// same oldest rows in front of newer ones. Returns how many expired
/// dispatches have a persisted relay after this sweep.
pub(super) fn escalate_expired_verification_dispatches(
    cas_dir: &std::path::Path,
    now: chrono::DateTime<chrono::Utc>,
) -> usize {
    /// New relays written per sweep; the sweep runs every 60 s.
    const RELAYS_PER_SWEEP: usize = 20;
    const KIND: &str = "verification_dispatch_expired";
    let since = now - chrono::Duration::hours(EXPIRED_DISPATCH_ESCALATION_WINDOW_HOURS);
    let expired = match cas_store::expired_verification_dispatches(cas_dir, since, now, 1000) {
        Ok(expired) => expired,
        Err(error) => {
            tracing::warn!(%error, "cas-7456: expired verification dispatches unavailable");
            return 0;
        }
    };
    let session = std::env::var("CAS_FACTORY_SESSION").ok();
    let supervisor_queue = crate::store::open_supervisor_queue_store(cas_dir).ok();
    let woken = |dispatch: &&cas_types::VerificationDispatch| {
        let key = worker_attention_relay_key(session.as_deref(), KIND, "supervisor", &dispatch.id);
        supervisor_queue.as_ref().is_some_and(|queue| {
            queue
                .get_by_transition_key(&key)
                .ok()
                .flatten()
                .is_some_and(|notification| notification.prompt_delivered_at.is_some())
        })
    };
    let (already, pending): (Vec<_>, Vec<_>) = expired.iter().partition(woken);
    already.len()
        + pending
        .into_iter()
        .take(RELAYS_PER_SWEEP)
        .filter(|dispatch| {
            let detail = format!(
                "Verification dispatch {} for {} passed its deadline {} with no verdict. Re-dispatch the verifier (or verify directly), then finish the close.",
                dispatch.id,
                dispatch.task_id,
                dispatch.deadline_at.to_rfc3339()
            );
            let overdue = (now - dispatch.deadline_at).num_seconds().max(0) as u64;
            matches!(
                enqueue_worker_attention_relay_detail(
                    cas_dir,
                    KIND,
                    "supervisor",
                    Some(&dispatch.task_id),
                    Some(overdue),
                    &detail,
                    &dispatch.id,
                ),
                WorkerAttentionRelayOutcome::Persisted { .. }
            )
        })
        .count()
}

fn enqueue_worker_attention_relay_for_episode(
    cas_dir: &std::path::Path,
    event: &crate::ui::factory::director::DirectorEvent,
    episode_start: Option<chrono::DateTime<chrono::Utc>>,
) -> WorkerAttentionRelayOutcome {
    if let crate::ui::factory::director::DirectorEvent::SupervisorStalled {
        next_step,
        occurrence,
        actionable_idle_secs,
        operator_page_episode,
    } = event
    {
        if let Some(episode) = operator_page_episode {
            page_operator_for_idle_supervisor(cas_dir, next_step, *episode);
        }
        if next_step.merged_close_blocked_relay_key().is_some() {
            // Each delivery owns one durable notice, even when the mixed queue
            // gains another task or the supervisor/daemon starts a new turn.
            let tasks = match next_step {
                crate::ui::factory::director::SupervisorActionableState::MergeCloseBlocked { tasks }
                | crate::ui::factory::director::SupervisorActionableState::MergeQueue {
                    merged_close_blocked: tasks, ..
                } => tasks,
                _ => unreachable!("merged relay key requires merged tasks"),
            };
            let mut merged = WorkerAttentionRelayOutcome::Pending;
            let mut pending = false;
            for task in tasks {
                let single = crate::ui::factory::director::SupervisorActionableState::MergeCloseBlocked {
                    tasks: vec![task.clone()],
                };
                let stable_key = single.merged_close_blocked_relay_key().unwrap();
                let outcome = enqueue_worker_attention_relay_detail_with_key(
                    cas_dir, "merged_close_blocked", "supervisor", Some(&task.task_id),
                    Some(*actionable_idle_secs),
                    |prefix| format!("Supervisor actionable-idle for {}m. {}",
                        actionable_idle_secs / 60, single.next_step_text_for(prefix)),
                    occurrence, Some(&stable_key),
                );
                pending |= matches!(outcome, WorkerAttentionRelayOutcome::Pending);
                merged = outcome;
            }
            if pending { merged = WorkerAttentionRelayOutcome::Pending; }

            // A mixed queue still needs the old genuinely-unmerged merge
            // demand. Its occurrence-based key preserves the prior refire
            // behavior, while the merged task remains stable-key deduped.
            if let Some(unmerged) = next_step.unmerged_merge_state() {
                let unmerged_detail = format!(
                    "Supervisor actionable-idle for {}m. {}",
                    actionable_idle_secs / 60,
                    unmerged.next_step_text()
                );
                let unmerged_outcome = enqueue_worker_attention_relay_detail(
                    cas_dir,
                    "supervisor_stalled",
                    "supervisor",
                    None,
                    Some(*actionable_idle_secs),
                    &unmerged_detail,
                    occurrence,
                );
                return if matches!(merged, WorkerAttentionRelayOutcome::Pending) {
                    unmerged_outcome
                } else {
                    merged
                };
            }
            return merged;
        }
        let detail = format!(
            "Supervisor actionable-idle for {}m. {}",
            actionable_idle_secs / 60,
            next_step.next_step_text()
        );
        return enqueue_worker_attention_relay_detail(
            cas_dir,
            "supervisor_stalled",
            "supervisor",
            None,
            Some(*actionable_idle_secs),
            &detail,
            occurrence,
        );
    }
    let (kind, worker, task_id, elapsed_secs) = match event {
        crate::ui::factory::director::DirectorEvent::WorkerIdle {
            worker,
            active_task: None,
        } => ("worker_idle", worker.as_str(), None, None),
        crate::ui::factory::director::DirectorEvent::WorkerStalled {
            worker,
            task_id,
            elapsed_secs,
            escalate: true,
        } => (
            "worker_stalled",
            worker.as_str(),
            Some(task_id.as_str()),
            Some(*elapsed_secs),
        ),
        _ => return WorkerAttentionRelayOutcome::NotApplicable,
    };
    let detail = match (kind, task_id, elapsed_secs) {
        ("worker_stalled", Some(task), Some(elapsed)) => format!(
            "Worker {worker} is stalled on {task} after {}m without activity.",
            elapsed / 60
        ),
        _ => format!("Worker {worker} is idle with no active task and needs supervisor attention."),
    };
    let Some(episode_start) = episode_start else {
        // No timestamp evidence is not a new occurrence. Never substitute
        // delivery time, which would manufacture a fresh alarm on every retry.
        return WorkerAttentionRelayOutcome::Pending;
    };
    let subject = task_id
        .map(|task| serde_json::json!([worker, task]).to_string())
        .unwrap_or_else(|| worker.to_string());
    enqueue_worker_attention_relay_detail(
        cas_dir,
        kind,
        worker,
        task_id,
        elapsed_secs,
        &detail,
        &attention_episode_key(kind, &subject, episode_start),
    )
}



/// Persist one confirmed worker stoppage through the same durable supervisor
/// relay used by director-detected idle and stall events.  This is deliberately
/// narrower than a normal-message delivery failure: callers invoke it only
/// after a bounded wake retry has also started no harness turn (cas-ac97).
/// GH #1163: one durable incident per worker per fixed five-minute UTC window.
pub(super) fn enqueue_worker_delivery_stalled_relay(
    cas_dir: &std::path::Path,
    worker: &str,
    message_id: i64,
    now: chrono::DateTime<chrono::Utc>,
) -> WorkerAttentionRelayOutcome {
    let detail = format!(
        "Worker {worker} started no harness turn after normal message {message_id} and its bounded retry."
    );
    enqueue_worker_attention_relay_detail(
        cas_dir,
        "worker_delivery_stalled",
        worker,
        None,
        None,
        &detail,
        // A durable worker/window identity coalesces concurrent probes and
        // retries after daemon restarts. Distinct workers retain separate keys.
        &format!("delivery-window:{}", now.timestamp().div_euclid(5 * 60)),
    )
}

pub(super) fn enqueue_usage_limit_relay(
    cas_dir: &std::path::Path, worker: &str, occurrence: &str, detail: &str,
) -> WorkerAttentionRelayOutcome {
    enqueue_worker_attention_relay_inner(cas_dir, "worker_unavailable", worker, None, None,
        |_| detail.to_owned(), occurrence, None, &[], true).0
}

/// Surface a terminal harness-side refusal even when its MCP child continues
/// heartbeating. The evidence is collected from the harness artifact, not
/// inferred from heartbeat age.
pub(super) fn enqueue_worker_unavailable_relay(
    cas_dir: &std::path::Path,
    worker: &str,
    occurrence: &str,
) -> WorkerAttentionRelayOutcome {
    let detail = format!(
        "Worker {worker}'s harness reported a terminal unavailable state while its process may still heartbeat."
    );
    enqueue_worker_attention_relay_detail(
        cas_dir,
        "worker_unavailable",
        worker,
        None,
        None,
        &detail,
        occurrence,
    )
}

/// cas-2ffe (GH #915): several workers' harnesses exited within the
/// correlation window. One wake names every worker and its exit status, keyed
/// on the first exit so a retry or daemon restart cannot repeat it.
pub(super) fn enqueue_correlated_worker_deaths_relay(
    cas_dir: &std::path::Path,
    group: &[super::queue_and_events::ObservedWorkerExit],
) -> WorkerAttentionRelayOutcome {
    enqueue_folded_worker_deaths_relay(cas_dir, group).outcome
}

/// cas-26c8: [`enqueue_correlated_worker_deaths_relay`] plus whether the
/// fold's durable row is recorded and its prompt enqueued, which decide what
/// happens to the held per-worker relays.
pub(super) fn enqueue_folded_worker_deaths_relay(
    cas_dir: &std::path::Path,
    group: &[super::queue_and_events::ObservedWorkerExit],
) -> FoldedRelayAttempt {
    let Some(first) = group.first() else {
        return FoldedRelayAttempt {
            outcome: WorkerAttentionRelayOutcome::NotApplicable,
            recorded: false,
            prompt_enqueued: false,
            recipient: None,
        };
    };
    let last = group.last().unwrap_or(first);
    let spread_ms = (last.at - first.at).num_milliseconds().max(0);
    let workers = group
        .iter()
        .map(|exit| format!("{} ({})", exit.worker, exit.status))
        .collect::<Vec<_>>()
        .join("; ");
    // cas-26c8 (ct2_p2_died_together_folds): this relay replaces the
    // per-worker worker_died relays, so each worker's tasks and cleaned tail
    // (12 lines / 600 chars, clean_death_tail) are listed here.
    let entries = group
        .iter()
        .map(|exit| {
            let Some(death) = exit.death.as_ref() else {
                return format!("- {} ({})", exit.worker, exit.status);
            };
            let list = |ids: &[String]| {
                if ids.is_empty() { "none".to_string() } else { ids.join(", ") }
            };
            let tail = death
                .exit
                .output_tail
                .as_deref()
                .map(|tail| {
                    let quoted = tail.lines().map(|line| format!("    > {line}")).collect::<Vec<_>>();
                    format!("\n  last output:\n{}", quoted.join("\n"))
                })
                .unwrap_or_default();
            format!(
                "- {} ({}): held tasks {}; parked {}{tail}",
                exit.worker,
                exit.status,
                list(&death.summary.held_task_ids),
                list(&death.summary.recovered_task_ids),
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let detail = format!(
        "{count} worker harnesses exited within {spread_ms} ms of each other, starting {start}: \
         {workers}. Treat this as one incident with a shared cause (host, harness binary, \
         account or network) before respawning. This relay replaces the per-worker \
         worker-died relays:\n{entries}",
        count = group.len(),
        start = first.at.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
    );
    let occurrence = format!("{}:{}", first.worker, first.at.timestamp_millis());
    let folded_incidents = group
        .iter()
        .filter_map(|exit| exit.death.as_ref())
        .map(|death| crate::mcp::tools::service::orphan_recovery::death_incident_key(&death.agent))
        .collect::<Vec<_>>();
    let (outcome, prompt_enqueued, recorded, recipient) = enqueue_worker_attention_relay_inner(
        cas_dir,
        "workers_died_together",
        &first.worker,
        None,
        None,
        |_| detail.clone(),
        &occurrence,
        None,
        &folded_incidents,
        true,
    );
    FoldedRelayAttempt {
        outcome,
        recorded,
        prompt_enqueued,
        recipient,
    }
}

/// cas-ff42 L2: inject the prompt of a fold's per-recipient copy (a live
/// Director's, say) to that recipient, as the per-worker prompts the fold
/// replaced would have been. The row is re-read on the write lane first.
pub(super) fn inject_fold_copy_prompt(
    cas_dir: &std::path::Path,
    notification_id: i64,
    payload: &serde_json::Value,
) -> WorkerAttentionRelayOutcome {
    use crate::mcp::tools::core::task::lifecycle::supervisor_push::LIFECYCLE_WAKE_SOURCE_PREFIX;
    use cas_store::{NotificationPriority, SupervisorQueueStore};
    let Ok(queue) = crate::store::open_supervisor_queue_store(cas_dir) else {
        return WorkerAttentionRelayOutcome::Pending;
    };
    match queue.get(notification_id) {
        Ok(Some(row)) if row.prompt_delivered_at.is_none() && row.processed_at.is_none() => {}
        _ => return WorkerAttentionRelayOutcome::NotApplicable,
    }
    let text = |key: &str| payload.get(key).and_then(serde_json::Value::as_str);
    let (Some(recipient), Some(worker), Some(detail)) =
        (text("fold_copy_for"), text("worker"), text("detail"))
    else {
        return WorkerAttentionRelayOutcome::NotApplicable;
    };
    let (Ok(agents), Ok(prompt_queue)) = (
        crate::store::open_agent_store(cas_dir),
        crate::store::open_prompt_queue_store(cas_dir),
    ) else {
        return WorkerAttentionRelayOutcome::Pending;
    };
    let Ok(agent) = agents.get(recipient) else {
        return WorkerAttentionRelayOutcome::NotApplicable;
    };
    let prefix = crate::harness_policy::agent_tool_prefix(&agent).unwrap_or("");
    let body = format!(
        "<worker-attention kind=\"workers_died_together\" worker=\"{worker}\" notification_id=\"{notification_id}\">\n{detail}\nRun `{prefix}factory action=worker_status` and reassign or recover the worker as needed.\n</worker-attention>"
    );
    if prompt_queue
        .enqueue_idempotent(
            &format!("{LIFECYCLE_WAKE_SOURCE_PREFIX}worker-attention:{notification_id}"),
            &agent.name,
            &body,
            agent.factory_session.as_deref(),
            Some(&format!("workers_died_together: {worker}")),
            Some(NotificationPriority::Critical),
            &format!("worker-attention-outbox:{notification_id}"),
            Some(&cas_store::QueueOrigin::Daemon),
        )
        .is_err()
    {
        return WorkerAttentionRelayOutcome::Pending;
    }
    super::delivery::wake_daemon_after_enqueue(cas_dir);
    let _ = queue.mark_prompt_delivered(notification_id);
    WorkerAttentionRelayOutcome::Persisted { notification_id }
}

/// cas-26c8 review M1: re-drive a folded relay whose durable row (with its
/// members already stamped) exists but whose prompt never reached the queue,
/// e.g. the daemon stopped between the two. The stored detail and occurrence
/// replay the same idempotent key.
pub(super) fn redrive_folded_worker_deaths_relay(
    cas_dir: &std::path::Path,
    notification_id: i64,
    payload: &serde_json::Value,
) -> WorkerAttentionRelayOutcome {
    use cas_store::SupervisorQueueStore;
    // Re-read on the write lane: a prompt injected since the sweep's read
    // means there is nothing to re-drive.
    match crate::store::open_supervisor_queue_store(cas_dir).map(|queue| queue.get(notification_id)) {
        Ok(Ok(Some(row))) if row.prompt_delivered_at.is_none() => {}
        _ => return WorkerAttentionRelayOutcome::NotApplicable,
    }
    let text = |key: &str| payload.get(key).and_then(serde_json::Value::as_str);
    let (Some(worker), Some(detail), Some(occurrence)) =
        (text("worker"), text("detail"), text("occurrence"))
    else {
        return WorkerAttentionRelayOutcome::NotApplicable;
    };
    let folded_incidents = payload
        .get("folded_incidents")
        .and_then(serde_json::Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    enqueue_worker_attention_relay_inner(
        cas_dir,
        "workers_died_together",
        worker,
        None,
        None,
        |_| detail.to_string(),
        occurrence,
        None,
        &folded_incidents,
        true,
    )
    .0
}

/// cas-4143: a teammate permission request CAS did not pre-approve has waited
/// past the threshold. Name the worker, tool, command and age, and give the
/// supervisor the approve/deny commands. One wake per request id.
pub(super) fn enqueue_worker_approval_pending_relay(
    cas_dir: &std::path::Path,
    request: &crate::factory_permission_relay::WorkerPermissionRequest,
) -> WorkerAttentionRelayOutcome {
    let worker = request.worker.as_str();
    let id = request.request_id.as_str();
    let detail = format!(
        "Worker {worker} has waited {age}s for a {tool} permission that Claude Code parked for a \
         team lead nobody plays (request {id}): `{command}`. CAS did not pre-approve this call. \
         Approve with `cas factory approve {worker} --request {id}` or deny with \
         `cas factory deny {worker} --request {id} --reason \"...\"`.",
        age = request.age_secs,
        tool = request.tool_name,
        command = request.command_excerpt,
    );
    enqueue_worker_attention_relay_detail(
        cas_dir,
        "worker_approval_pending",
        worker,
        None,
        Some(request.age_secs),
        &detail,
        id,
    )
}

/// A parked delivery whose PR was ejected must wake the supervisor *and* put
/// a durable instruction in the delivering worker's inbox. The occurrence is
/// a failed merge-group run ID when available, so a requeue naturally arms a
/// new episode while repeated polls/restarts stay idempotent.
///
/// cas-e369: an empty `worker` is a supervisor-owned epic PR, which has no
/// worker inbox row. cas-fd0a: `failing_job` names the failed job and step;
/// nothing is re-enqueued automatically.
pub(super) fn enqueue_merge_queue_ejection_relay(
    cas_dir: &std::path::Path,
    task_id: &str,
    worker: &str,
    pr_number: u64,
    failed_run_id: Option<u64>,
    occurrence: &str,
    failing_job: Option<&str>,
) -> WorkerAttentionRelayOutcome {
    use crate::mcp::tools::core::task::lifecycle::supervisor_push::{
        LIFECYCLE_WAKE_SOURCE_PREFIX, resolve_notice_recipient,
    };
    use cas_store::{NotificationPriority, NotifyIdempotentResult};

    let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
    let Ok(agent_store) = crate::store::open_agent_store(cas_dir) else {
        return WorkerAttentionRelayOutcome::Pending;
    };
    let supervisor = resolve_notice_recipient(agent_store.as_ref(), factory_session.as_deref());
    let kind = if worker.is_empty() {
        "Epic PR"
    } else {
        "Delivery PR"
    };
    let mut detail = match failed_run_id {
        Some(run_id) => format!(
            "{kind} #{pr_number} for {task_id} left the merge queue after failed merge_group run {run_id}."
        ),
        None => format!(
            "{kind} #{pr_number} for {task_id} left the merge queue without merging (dequeued or auto-merge disarmed)."
        ),
    };
    if let Some(job) = failing_job {
        detail.push_str(&format!("\nFailing job: {job}."));
    }
    let key = format!(
        "merge-queue-ejection:{}:{task_id}:{pr_number}:{occurrence}",
        factory_session.as_deref().unwrap_or("")
    );
    let Ok(supervisor_queue) = crate::store::open_supervisor_queue_store(cas_dir) else {
        return WorkerAttentionRelayOutcome::Pending;
    };
    let payload = serde_json::json!({
        "kind": "merge_queue_ejected",
        "worker": worker,
        "task_id": task_id,
        "pr_number": pr_number,
        "failed_run_id": failed_run_id,
        "detail": detail,
        "occurrence": occurrence,
        "supervisor_owned": worker.is_empty(),
        "failing_job": failing_job,
    })
    .to_string();
    let notification_id = match supervisor_queue.notify_idempotent(
        &supervisor.id,
        "merge_queue_ejected",
        &payload,
        NotificationPriority::High,
        &key,
    ) {
        Ok(NotifyIdempotentResult::Created(id)) => id,
        Ok(NotifyIdempotentResult::AlreadyExists {
            id,
            prompt_delivered: true,
        }) if supervisor.id != "operator" => {
            return WorkerAttentionRelayOutcome::Persisted {
                notification_id: id,
            };
        }
        Ok(NotifyIdempotentResult::AlreadyExists {
            id,
            ..
        }) => id,
        Err(error) => {
            tracing::error!(task_id, pr_number, %error, "merge-queue ejection durable enqueue failed");
            return WorkerAttentionRelayOutcome::Pending;
        }
    };
    let Ok(prompt_queue) = crate::store::open_prompt_queue_store(cas_dir) else {
        return WorkerAttentionRelayOutcome::Pending;
    };
    let next_step = "\nInspect the failed run and requeue or request changes.";
    let supervisor_body = format!(
        "<worker-attention kind=\"merge_queue_ejected\" worker=\"{worker}\" task_id=\"{task_id}\" notification_id=\"{notification_id}\">\n{detail}{next_step}\n</worker-attention>"
    );
    if supervisor.id == "operator" {
        // A departed factory's direct prompt cannot reach a Commander on
        // another daemon. Reuse the project operator outbox and its existing
        // adoption tick, including when a prior live-owner relay was stamped.
        if let Err(error) = crate::supervisor_notices::enqueue(
            cas_dir,
            &supervisor,
            "merge_queue_ejected",
            &format!("{detail}\nNo live supervisor is available.{next_step}"),
            &format!("{task_id}:{pr_number}:{occurrence}"),
        ) {
            tracing::warn!(task_id, pr_number, %error, "merge-queue operator escalation remains pending");
            return WorkerAttentionRelayOutcome::Pending;
        }
    } else {
        let source = format!("{LIFECYCLE_WAKE_SOURCE_PREFIX}worker-attention:{notification_id}");
        if prompt_queue.enqueue_idempotent(
            &source,
            &supervisor.name,
            &supervisor_body,
            supervisor.session.as_deref(),
            Some(&format!("merge queue ejected: {task_id} PR #{pr_number}")),
            Some(NotificationPriority::High),
            &format!("merge-queue-ejection-outbox:{notification_id}"),
            Some(&cas_store::QueueOrigin::Daemon),
        ).is_err() {
            return WorkerAttentionRelayOutcome::Pending;
        }
    }
    if !worker.is_empty() && prompt_queue.enqueue_idempotent(
        "merge-queue-ejection",
        worker,
        &format!("{detail}\nWait for the supervisor's merge recovery instructions before starting other work."),
        factory_session.as_deref(),
        Some(&format!("merge queue ejected: {task_id}")),
        Some(NotificationPriority::High),
        &format!("merge-queue-ejection-worker:{key}"),
        Some(&cas_store::QueueOrigin::Daemon),
    ).is_err() {
        return WorkerAttentionRelayOutcome::Pending;
    }
    let Ok(task_store) = crate::store::open_task_store_cached(cas_dir) else {
        return WorkerAttentionRelayOutcome::Pending;
    };
    let Ok(mut task) = task_store.get(task_id) else {
        return WorkerAttentionRelayOutcome::Pending;
    };
    let marker = format!("merge-queue-ejection occurrence={occurrence}");
    if !task.notes.contains(&marker) {
        let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M");
        let note = format!("[{timestamp}] 🚫 BLOCKER {detail} ({marker}).");
        task.notes = if task.notes.is_empty() {
            note
        } else {
            format!("{}\n\n{note}", task.notes)
        };
        task.updated_at = chrono::Utc::now();
        if let Err(error) = task_store.update(&task) {
            tracing::warn!(task_id, %error, "merge-queue ejection task note write failed");
            return WorkerAttentionRelayOutcome::Pending;
        }
    }
    // Keep the durable outbox replayable until the task-note receipt exists;
    // otherwise a one-off store failure would leave an alert with no audit
    // trail while future polls short-circuited as already delivered.
    if supervisor_queue
        .mark_prompt_delivered(notification_id)
        .is_err()
    {
        return WorkerAttentionRelayOutcome::Pending;
    }
    super::delivery::wake_daemon_after_enqueue(cas_dir);
    WorkerAttentionRelayOutcome::Persisted { notification_id }
}

/// Surface a failed required PR-lane check through the same durable
/// supervisor-facing worker-attention channel as merge-queue ejections.
/// `PrLaneFailure::dedupe_key` binds retries to the PR head, so a plain CI
/// rerun stays quiet while a corrective push starts a new episode.
pub(super) fn enqueue_pr_lane_failure_relay(
    cas_dir: &std::path::Path,
    failure: &super::ci_watch::PrLaneFailure,
) -> WorkerAttentionRelayOutcome {
    let detail = format!(
        "Delivery PR #{} for {} failed required check {} in run {} at {} (head {}).",
        failure.pr_number,
        failure.task_id,
        failure.check_name,
        failure.run_id,
        failure.run_url,
        failure.head_sha,
    );
    let key = failure.dedupe_key();
    enqueue_worker_attention_relay_detail_with_key(
        cas_dir,
        "pr_lane_failed",
        &failure.worker,
        Some(&failure.task_id),
        None,
        |_| detail.clone(),
        &key,
        Some(&key),
    )
}

/// Wake the delivering worker after GitHub confirms that its main-targeted PR
/// merged. The source and dedupe key contain the immutable PR/merge receipt so
/// repeated polls and daemon restarts cannot duplicate the lifecycle wake.
fn enqueue_delivery_pr_merged_wake(
    cas_dir: &std::path::Path,
    task_id: &str,
    worker: &str,
    pr_number: u64,
    merge_commit: &str,
) -> Result<(), String> {
    use crate::mcp::tools::core::task::lifecycle::supervisor_push::LIFECYCLE_WAKE_SOURCE_PREFIX;
    use cas_store::NotificationPriority;

    let queue = crate::store::open_prompt_queue_store(cas_dir)
        .map_err(|error| format!("could not open prompt queue: {error}"))?;
    let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
    let key = format!("delivery-pr-merged:{task_id}:{pr_number}:{merge_commit}");
    let source = format!("{LIFECYCLE_WAKE_SOURCE_PREFIX}{key}");
    let prompt = crate::prompt_revalidation::delivery_pr_merged_envelope(
        task_id,
        worker,
        pr_number,
        merge_commit,
    );
    queue
        .enqueue_idempotent(
            &source,
            worker,
            &prompt,
            factory_session.as_deref(),
            Some(&format!("PR #{pr_number} merged: close {task_id}")),
            Some(NotificationPriority::High),
            &key,
            Some(&cas_store::QueueOrigin::Daemon),
        )
        .map_err(|error| format!("could not enqueue delivery merge wake: {error}"))?;
    super::delivery::wake_daemon_after_enqueue(cas_dir);
    Ok(())
}

/// Drain the durable lifecycle outbox once (cas-ecff). Runs on the daemon's
/// store worker (GH #1165).
fn drain_lifecycle_outbox_once(cas_dir: &std::path::Path) {
    let Ok(sq) = crate::store::open_supervisor_queue_store(cas_dir) else {
        return;
    };
    let Ok(pq) = crate::store::open_prompt_queue_store(cas_dir) else {
        return;
    };
    let Ok(agents) = crate::store::open_agent_store(cas_dir) else {
        return;
    };
    match crate::mcp::tools::core::task::lifecycle::supervisor_push::drain_lifecycle_outbox_with_agents(
        sq.as_ref(),
        pq.as_ref(),
        agents.as_ref(),
        50,
    ) {
        Ok(report) if report.recovered > 0 || report.failed > 0 => {
            tracing::info!(
                recovered = report.recovered,
                failed = report.failed,
                attempted = report.attempted,
                "lifecycle outbox drain"
            );
        }
        Err(e) => {
            tracing::warn!(error = %e, "lifecycle outbox drain failed");
        }
        _ => {}
    }
}

/// Parked deliveries targeting `main`, for the CI watcher. Runs on the
/// blocking pool with the GitHub calls (GH #1165).
fn awaiting_merge_deliveries(
    cas_dir: &std::path::Path,
) -> Vec<super::ci_watch::AwaitingMergeDelivery> {
    crate::store::open_task_store_cached(cas_dir)
        .ok()
        .and_then(|store| store.list(Some(cas_types::TaskStatus::AwaitingMerge)).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|task| {
            let worker = task.assignee?;
            let work_target = task.deliverables.work_target.as_ref()?;
            if work_target.target_branch != "main" {
                return None;
            }
            let branch = task
                .deliverables
                .parked_branch
                .or(task.branch)
                .unwrap_or_else(|| format!("factory/{worker}"));
            Some(super::ci_watch::AwaitingMergeDelivery {
                task_id: task.id,
                worker,
                branch,
                branch_tip: task.deliverables.factory_branch_anchor,
                pr_number: task.deliverables.delivery_pr_number,
            })
        })
        .collect()
}

/// Persist one completed CI-watch poll: PR observations, merge timeouts,
/// merge-queue ejections, PR-lane failures and red-run wakes. Runs on the
/// daemon's store worker, never on the UI loop (GH #1165).
fn persist_ci_watch_results(
    cas_dir: &std::path::Path,
    session_name: &str,
    delivery_pr_observations: Vec<super::ci_watch::DeliveryPrObservation>,
    ejections: Vec<(
        super::ci_watch::MergeQueueEjection,
        Option<String>,
    )>,
    pr_lane_failures: Vec<super::ci_watch::PrLaneFailure>,
    failures: Vec<super::ci_watch::CiFailure>,
) {
    for observation in delivery_pr_observations {
        match apply_delivery_pr_observation(cas_dir, &observation) {
            Ok(()) => tracing::info!(
                task_id = %observation.task_id,
                pr_number = observation.pr_number,
                merge_commit = ?observation.merge_commit,
                "recorded delivery PR observation"
            ),
            Err(error) => tracing::warn!(
                task_id = %observation.task_id,
                pr_number = observation.pr_number,
                %error,
                "could not persist delivery PR observation"
            ),
        }
    }
    relay_delivery_pr_merge_timeouts(cas_dir);
    for (ejection, failing_job) in ejections {
        match enqueue_merge_queue_ejection_relay(
            cas_dir,
            &ejection.task_id,
            &ejection.worker,
            ejection.pr_number,
            ejection.failed_run_id,
            &ejection.occurrence,
            failing_job.as_deref(),
        ) {
            WorkerAttentionRelayOutcome::Persisted { notification_id } => {
                tracing::warn!(
                    task_id = %ejection.task_id,
                    pr_number = ejection.pr_number,
                    failed_run_id = ?ejection.failed_run_id,
                    notification_id,
                    "queued durable merge-queue ejection relay for supervisor and worker"
                )
            }
            WorkerAttentionRelayOutcome::Pending => tracing::warn!(
                task_id = %ejection.task_id,
                pr_number = ejection.pr_number,
                "merge-queue ejection relay remains pending"
            ),
            WorkerAttentionRelayOutcome::NotApplicable => {}
        }
    }
    for failure in pr_lane_failures {
        match enqueue_pr_lane_failure_relay(cas_dir, &failure) {
            WorkerAttentionRelayOutcome::Persisted { notification_id } => {
                tracing::warn!(
                    task_id = %failure.task_id,
                    worker = %failure.worker,
                    pr_number = failure.pr_number,
                    head_sha = %failure.head_sha,
                    run_id = failure.run_id,
                    notification_id,
                    "queued durable PR-lane failure relay for supervisor"
                )
            }
            WorkerAttentionRelayOutcome::Pending => tracing::warn!(
                task_id = %failure.task_id,
                pr_number = failure.pr_number,
                head_sha = %failure.head_sha,
                "PR-lane failure relay remains pending"
            ),
            WorkerAttentionRelayOutcome::NotApplicable => {}
        }
    }
    if failures.is_empty() {
        return;
    }
    let queue = match crate::store::open_prompt_queue_store(cas_dir) {
        Ok(queue) => queue,
        Err(error) => {
            tracing::warn!(%error, "could not open prompt queue for CI red-run relays");
            return;
        }
    };
    for failure in failures {
        match super::ci_watch::emit_failure(queue.as_ref(), session_name, &failure) {
            Ok(true) => {
                super::delivery::wake_daemon_after_enqueue(cas_dir);
                tracing::warn!(
                    branch = %failure.branch,
                    head_sha = %failure.head_sha,
                    run_url = %failure.run_url,
                    failing_job = %failure.failing_job,
                    "queued CI red-run lifecycle wake for supervisor"
                )
            }
            Ok(false) => tracing::debug!(
                branch = %failure.branch,
                head_sha = %failure.head_sha,
                "suppressed duplicate CI red-run relay"
            ),
            Err(error) => {
                tracing::warn!(%error, "failed to queue CI red-run relay")
            }
        }
    }
}

/// Persist the PR identity as soon as it is discovered, then persist the merge
/// receipt only after the worker wake is durable. A failed prompt write leaves
/// the merge unrecorded so the next GitHub poll retries the complete handoff.
fn apply_delivery_pr_observation(
    cas_dir: &std::path::Path,
    observation: &super::ci_watch::DeliveryPrObservation,
) -> Result<(), String> {
    let store = crate::store::open_task_store_cached(cas_dir)
        .map_err(|error| format!("could not open task store: {error}"))?;
    let mut task = store
        .get(&observation.task_id)
        .map_err(|error| format!("could not read task: {error}"))?;
    let mut changed = false;
    if task.deliverables.delivery_pr_number != Some(observation.pr_number) {
        task.deliverables.delivery_pr_number = Some(observation.pr_number);
        changed = true;
    }
    let Some(merge_commit) = observation.merge_commit.as_deref() else {
        if changed {
            store
                .update(&task)
                .map_err(|error| format!("could not record delivery PR: {error}"))?;
        }
        return Ok(());
    };
    let already_recorded = task.deliverables.delivery_pr_merge_commit.as_deref()
        == Some(merge_commit)
        && task.deliverables.delivery_pr_merged_at.is_some();
    if !already_recorded {
        if !task.is_terminal() {
            enqueue_delivery_pr_merged_wake(
                cas_dir,
                &observation.task_id,
                &observation.worker,
                observation.pr_number,
                merge_commit,
            )?;
        }
        task.deliverables.delivery_pr_merge_commit = Some(merge_commit.to_string());
        task.deliverables.delivery_pr_merged_at = Some(chrono::Utc::now());
        changed = true;
    }
    if changed {
        store
            .update(&task)
            .map_err(|error| format!("could not record delivery PR merge: {error}"))?;
    }
    Ok(())
}

/// Give the worker a bounded chance to close after the typed merge wake. If it
/// remains open, the existing durable supervisor attention lane tells the
/// supervisor exactly which PR/receipt needs recovery. Its stable key makes
/// this a single escalation for the merge episode.
fn relay_delivery_pr_merge_timeouts(cas_dir: &std::path::Path) {
    let Ok(store) = crate::store::open_task_store_cached(cas_dir) else {
        return;
    };
    let Ok(tasks) = store.list(None) else {
        return;
    };
    let now = chrono::Utc::now();
    for task in tasks {
        let (Some(pr_number), Some(merge_commit), Some(merged_at), Some(worker)) = (
            task.deliverables.delivery_pr_number,
            task.deliverables.delivery_pr_merge_commit.as_deref(),
            task.deliverables.delivery_pr_merged_at,
            task.assignee.as_deref(),
        ) else {
            continue;
        };
        let Ok(age) = (now - merged_at).to_std() else {
            continue;
        };
        if task.is_terminal() || age < super::ci_watch::DELIVERY_MERGE_CLOSE_GRACE {
            continue;
        }
        let detail = format!(
            "Delivery PR #{pr_number} for {} merged at {merge_commit}, but task remains {} after {}m. The worker was already told to close; inspect or recover the task.",
            task.id,
            task.status,
            age.as_secs() / 60,
        );
        let occurrence = format!("delivery-pr-merged:{}:{pr_number}:{merge_commit}", task.id);
        match enqueue_worker_attention_relay_detail_with_key(
            cas_dir,
            "delivery_pr_merge_timeout",
            worker,
            Some(&task.id),
            Some(age.as_secs()),
            |_| detail.clone(),
            &occurrence,
            Some(&occurrence),
        ) {
            WorkerAttentionRelayOutcome::Persisted { notification_id } => tracing::warn!(
                task_id = %task.id,
                pr_number,
                merge_commit,
                notification_id,
                "delivery PR remained open after merge wake grace"
            ),
            WorkerAttentionRelayOutcome::Pending => tracing::warn!(
                task_id = %task.id,
                pr_number,
                "delivery PR merge timeout relay remains pending"
            ),
            WorkerAttentionRelayOutcome::NotApplicable => {}
        }
    }
}

/// The fail-safe for supervisor traffic that is NOT wake-eligible (cas-d9a8).
///
/// A worker's blocker, verification handoff or plan carries no CAS-emitted
/// envelope, so it stays inbox-only — waking on those would mean waking on a
/// keyword, which is the class of check cas-d9a8 removed. But "inbox-only"
/// became "discovered by polling", and on 2026-09-04 that hid a verification
/// dispatch-id handoff that blocked a task close for hours.
///
/// So the messages themselves stay quiet and CAS says the COUNT out loud
/// instead: one relay naming how many supervisor-addressed messages have gone
/// unread past their delivery-stalled threshold, and who sent the oldest.
///
/// `oldest_id` is the dedupe key, which sets the cadence for free: the relay
/// fires once per distinct backlog head. A supervisor who reads the oldest
/// moves the head and gets told about what is still waiting; a supervisor who
/// reads nothing is not told twice about the same message.
pub(super) fn enqueue_supervisor_unread_relay(
    cas_dir: &std::path::Path,
    oldest_sender: &str,
    oldest_id: i64,
    unread: usize,
) -> WorkerAttentionRelayOutcome {
    let detail = if unread == 1 {
        format!(
            "1 message addressed to you has gone unread past its delivery threshold — message {oldest_id}, from {oldest_sender}. No message-specific read or wake confirmation has been recorded."
        )
    } else {
        format!(
            "{unread} messages addressed to you have gone unread past their delivery threshold. The oldest is message {oldest_id}, from {oldest_sender}. No message-specific read or wake confirmation has been recorded for this backlog."
        )
    };
    let key = format!("supervisor-unread:{oldest_id}");
    enqueue_worker_attention_relay_detail_with_key(
        cas_dir,
        "supervisor_unread",
        oldest_sender,
        None,
        None,
        |_| detail.clone(),
        &key,
        Some(&key),
    )
}

/// A relay is consumed by an in-memory detector only after both durable lanes
/// are confirmed. `Pending` deliberately leaves that detector eligible for a
/// retry; the occurrence key makes the replay idempotent after a partial write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WorkerAttentionRelayOutcome {
    Persisted { notification_id: i64 },
    Pending,
    NotApplicable,
}

/// The daemon observed one original physical write past its outcome bound.
/// Retry the same durable episode, never the prompt's PTY bytes.
pub(super) fn enqueue_blocked_pty_write_relay(
    cas_dir: &std::path::Path,
    pane: &str,
    detail: &str,
    occurrence: &str,
) -> WorkerAttentionRelayOutcome {
    enqueue_worker_attention_relay_detail(
        cas_dir,
        "pty_write_stalled",
        pane,
        None,
        None,
        detail,
        occurrence,
    )
}

fn enqueue_worker_attention_relay_detail(
    cas_dir: &std::path::Path,
    kind: &str,
    worker: &str,
    task_id: Option<&str>,
    elapsed_secs: Option<u64>,
    detail: &str,
    occurrence: &str,
) -> WorkerAttentionRelayOutcome {
    enqueue_worker_attention_relay_detail_with_key(
        cas_dir,
        kind,
        worker,
        task_id,
        elapsed_secs,
        |_| detail.to_string(),
        occurrence,
        None,
    )
}

fn enqueue_worker_attention_relay_detail_with_key(
    cas_dir: &std::path::Path,
    kind: &str,
    worker: &str,
    task_id: Option<&str>,
    elapsed_secs: Option<u64>,
    detail: impl Fn(&str) -> String,
    occurrence: &str,
    stable_key: Option<&str>,
) -> WorkerAttentionRelayOutcome {
    enqueue_worker_attention_relay_inner(
        cas_dir, kind, worker, task_id, elapsed_secs, detail, occurrence, stable_key, &[], true,
    )
    .0
}

/// cas-26c8 / cas-ff42: one folded-relay attempt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FoldedRelayAttempt {
    pub outcome: WorkerAttentionRelayOutcome,
    /// The fold's durable row is committed, which stamped its members in the
    /// same transaction. From here the death sweep owns its prompt.
    pub recorded: bool,
    pub prompt_enqueued: bool,
    /// cas-ff42 review: who the fold went to, for whether the death sweep
    /// re-drives its prompt.
    pub recipient: Option<String>,
}

/// The durable dedupe key of one worker-attention episode (its
/// `supervisor_queue.transition_key`), shared by the relay writer and the
/// sweeps that ask whether an episode was already relayed (cas-b080).
fn worker_attention_relay_key(
    factory_session: Option<&str>,
    kind: &str,
    worker: &str,
    occurrence: &str,
) -> String {
    format!(
        "worker-attention:{}:{kind}:{worker}:{occurrence}",
        factory_session.unwrap_or("")
    )
}

/// The relay, whether its supervisor prompt is enqueued, and whether its
/// durable row is recorded. A `Pending` outcome whose prompt is enqueued
/// failed only to stamp the durable row: the supervisor will still hear it
/// (cas-26c8 L2). A recorded row the prompt never reached is re-driven by the
/// death sweep (cas-ff42 L1).
#[allow(clippy::too_many_arguments)]
fn enqueue_worker_attention_relay_inner(
    cas_dir: &std::path::Path,
    kind: &str,
    worker: &str,
    task_id: Option<&str>,
    elapsed_secs: Option<u64>,
    detail: impl Fn(&str) -> String,
    occurrence: &str,
    stable_key: Option<&str>,
    folded_incidents: &[String],
    needs_recovery: bool,
) -> (WorkerAttentionRelayOutcome, bool, bool, Option<String>) {
    use crate::mcp::tools::core::task::lifecycle::supervisor_push::{
        LIFECYCLE_WAKE_SOURCE_PREFIX, resolve_notice_recipient,
    };
    use cas_store::{NotificationPriority, NotifyIdempotentResult};

    let factory_session = std::env::var("CAS_FACTORY_SESSION").ok();
    let Ok(agent_store) = crate::store::open_agent_store(cas_dir) else {
        tracing::warn!("worker attention relay skipped: agent store unavailable");
        return (WorkerAttentionRelayOutcome::Pending, false, false, None);
    };
    let supervisor = resolve_notice_recipient(agent_store.as_ref(), factory_session.as_deref());
    let Ok(supervisor_queue) = crate::store::open_supervisor_queue_store(cas_dir) else {
        tracing::warn!(worker = %worker, kind, "worker attention relay skipped: supervisor queue unavailable");
        return (WorkerAttentionRelayOutcome::Pending, false, false, Some(supervisor.id.clone()));
    };
    // `occurrence` identifies the detected episode, not this delivery attempt.
    // A retry after a durable-only write must reuse it across daemon restarts.
    let key = stable_key
        .map(|key| {
            format!(
                "worker-attention:{}:{kind}:{key}",
                factory_session.as_deref().unwrap_or("")
            )
        })
        .unwrap_or_else(|| {
            worker_attention_relay_key(factory_session.as_deref(), kind, worker, occurrence)
        });
    let payload = serde_json::json!({
        "kind": kind,
        "worker": worker,
        "task_id": task_id,
        "elapsed_secs": elapsed_secs,
        "detail": detail(""),
        "factory_session": factory_session,
        "occurrence": occurrence,
        "folded_incidents": folded_incidents,
        "needs_recovery": needs_recovery,
    })
    .to_string();
    // cas-26c8: a folded multi-worker death replaces Critical worker_died
    // relays, so it keeps their priority.
    let priority = if kind == "workers_died_together" {
        NotificationPriority::Critical
    } else {
        NotificationPriority::High
    };
    // cas-26c8 review M1: a folded relay is recorded in the same transaction
    // that stamps the per-worker worker_died rows it replaces.
    let recorded = if folded_incidents.is_empty() {
        supervisor_queue.notify_idempotent(&supervisor.id, kind, &payload, priority, &key)
    } else {
        supervisor_queue.notify_idempotent_folding(
            &supervisor.id,
            kind,
            &payload,
            priority,
            &key,
            "worker_died",
            folded_incidents,
        )
    };
    let notification_id = match recorded {
        Ok(NotifyIdempotentResult::Created(id)) => id,
        Ok(NotifyIdempotentResult::AlreadyExists {
            id,
            prompt_delivered: true,
        }) => {
            return (
                WorkerAttentionRelayOutcome::Persisted {
                    notification_id: id,
                },
                true,
                true,
                Some(supervisor.id.clone()),
            );
        }
        Ok(NotifyIdempotentResult::AlreadyExists {
            id,
            prompt_delivered: false,
        }) => id,
        Err(error) => {
            tracing::error!(worker = %worker, kind, %error, "worker attention relay durable enqueue failed");
            return (WorkerAttentionRelayOutcome::Pending, false, false, Some(supervisor.id.clone()));
        }
    };
    let Ok(prompt_queue) = crate::store::open_prompt_queue_store(cas_dir) else {
        tracing::error!(worker = %worker, kind, notification_id, "worker attention relay left pending: prompt queue unavailable");
        return (WorkerAttentionRelayOutcome::Pending, false, true, Some(supervisor.id.clone()));
    };
    // Resolve the registered receiver at this enqueue attempt, including retries
    // after a durable-only write. Missing harness evidence stays neutral; the
    // daemon's own process environment does not describe the receiving agent.
    let prefix = agent_store
        .get(&supervisor.id)
        .ok()
        .as_ref()
        .and_then(crate::harness_policy::agent_tool_prefix)
        .unwrap_or("");
    let detail = detail(prefix);
    let recovery = if needs_recovery {
        format!("\nRun `{prefix}factory action=worker_status` and reassign or recover the worker as needed.")
    } else {
        String::new()
    };
    let mut body = format!(
        "<worker-attention kind=\"{kind}\" worker=\"{worker}\" notification_id=\"{notification_id}\">\n{detail}{recovery}\n</worker-attention>"
    );
    // cas-207e: ready tasks or merges waiting on an actionable-idle
    // supervisor are a blocker, not routine: idle workers burn wall clock.
    if kind == "supervisor_stalled" {
        body = crate::prompt_revalidation::attach_blocker_envelope(&body, "ready work", task_id);
    } else if kind == "pty_write_stalled" {
        body = crate::prompt_revalidation::attach_blocker_envelope(&body, "PTY write blocked", task_id);
    }
    if supervisor.id == "operator" {
        body = match crate::supervisor_notices::operator_prompt(&body, kind) {
            Ok(body) => body,
            Err(_) => return (WorkerAttentionRelayOutcome::Pending, false, true, Some(supervisor.id.clone())),
        };
    }
    let source = format!("{LIFECYCLE_WAKE_SOURCE_PREFIX}worker-attention:{notification_id}");
    if let Err(error) = prompt_queue.enqueue_idempotent(
        &source,
        &supervisor.name,
        &body,
        supervisor.session.as_deref(),
        Some(&format!("{kind}: {worker}")),
        Some(priority),
        &format!("worker-attention-outbox:{notification_id}"),
        Some(&cas_store::QueueOrigin::Daemon),
    ) {
        tracing::error!(worker = %worker, kind, notification_id, %error, "worker attention relay prompt enqueue failed");
        return (WorkerAttentionRelayOutcome::Pending, false, true, Some(supervisor.id.clone()));
    }
    super::delivery::wake_daemon_after_enqueue(cas_dir);
    if let Err(error) = supervisor_queue.mark_prompt_delivered(notification_id) {
        tracing::warn!(notification_id, %error, "worker attention relay prompt stamp failed; idempotent replay remains safe");
        return (WorkerAttentionRelayOutcome::Pending, true, true, Some(supervisor.id.clone()));
    }
    (WorkerAttentionRelayOutcome::Persisted { notification_id }, true, true, Some(supervisor.id.clone()))
}

#[cfg(test)]
mod worker_attention_tests {
    use super::*;
    use cas_types::{Agent, AgentRole, AgentStatus};

    fn register_supervisor(cas_dir: &std::path::Path, session: &str) {
        let agents = crate::store::open_agent_store(cas_dir).unwrap();
        let mut supervisor = Agent::new("supervisor-id".to_string(), "supervisor".to_string());
        supervisor.role = AgentRole::Supervisor;
        supervisor.status = AgentStatus::Active;
        supervisor.factory_session = Some(session.to_string());
        agents.register(&supervisor).unwrap();
    }

    #[test]
    fn ct2_p2_second_idle_episode_relays() {
        use crate::ui::factory::director::{
            AgentSummary, DirectorData, DirectorEvent, DirectorEventDetector,
        };
        let _env =
            crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "ct2-idle")]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "ct2-idle");
        let now = chrono::Utc::now();
        let tick = std::time::Instant::now();
        let mut data = DirectorData {
            ready_tasks: vec![],
            in_progress_tasks: vec![],
            epic_tasks: vec![],
            agents: vec![AgentSummary {
                id: "worker-id".into(),
                name: "gold-fox".into(),
                status: AgentStatus::Active,
                registered_at: now - chrono::Duration::minutes(10),
                current_task: Some("cas-work".into()),
                latest_activity: None,
                last_heartbeat: None,
                pending_messages: 0,
                pending_supervisor_messages: 0,
                latest_supervisor_message_at: None,
                active_lease: None,
                effort: None,
                qa_reviewer_between_rounds: false,
            }],
            activity: vec![],
            agent_id_to_name: [("worker-id".into(), "gold-fox".into())]
                .into_iter()
                .collect(),
            changes: vec![],
            git_loaded: true,
            reminders: vec![],
            epic_closed_counts: HashMap::new(),
            start_gated_task_ids: Default::default(),
        };
        let mut detector = DirectorEventDetector::new(vec!["gold-fox".into()], "supervisor".into());
        detector.initialize(&data);
        let mut starts = vec![];
        for episode in 0..2 {
            data.agents[0].current_task = None;
            let mut idle = None;
            for offset in 0..2 {
                let seconds = episode * 600 + offset;
                let events = detector.detect_changes_at(
                    &data,
                    None,
                    tick + Duration::from_secs(seconds),
                    now + chrono::Duration::seconds(seconds as i64),
                );
                idle = events
                    .into_iter()
                    .find(|event| matches!(event, DirectorEvent::WorkerIdle { .. }))
                    .or(idle);
            }
            let event = idle.expect("sustained idle emits after debounce");
            let start = detector
                .worker_attention_episode_start(&event)
                .expect("idle episode start");
            starts.push(start);
            let first = enqueue_worker_attention_relay_for_episode(&cas_dir, &event, Some(start));
            let duplicate =
                enqueue_worker_attention_relay_for_episode(&cas_dir, &event, Some(start));
            assert!(matches!(
                first,
                WorkerAttentionRelayOutcome::Persisted { .. }
            ));
            assert_eq!(first, duplicate, "same episode must dedupe");
            data.agents[0].current_task = Some("cas-work".into());
            detector.detect_changes_at(
                &data,
                None,
                tick + Duration::from_secs(episode * 600 + 10),
                now + chrono::Duration::seconds((episode * 600 + 10) as i64),
            );
        }
        assert_ne!(starts[0], starts[1]);
        let notifications = crate::store::open_supervisor_queue_store(&cas_dir)
            .unwrap()
            .peek("supervisor-id", 10)
            .unwrap();
        assert_eq!(
            notifications.len(),
            2,
            "the second idle episode must produce a second durable relay"
        );
        let prompts = crate::store::open_prompt_queue_store(&cas_dir)
            .unwrap()
            .peek_all(10)
            .unwrap();
        assert_eq!(
            prompts.len(),
            2,
            "each episode reaches the real supervisor lane once"
        );
    }

    #[test]
    fn ct2_p2_second_stall_episode_relays() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[("CAS_FACTORY_SESSION", "ct2-stall")]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "ct2-stall");
        let start = chrono::Utc::now();
        let event = crate::ui::factory::director::DirectorEvent::WorkerStalled {
            worker: "gold-fox".into(), task_id: "cas-same-task".into(), elapsed_secs: 600, escalate: true,
        };
        for epoch in [start, start + chrono::Duration::minutes(10)] {
            let sent = enqueue_worker_attention_relay_for_episode(&cas_dir, &event, Some(epoch));
            let retry = enqueue_worker_attention_relay_for_episode(&cas_dir, &event, Some(epoch));
            assert!(matches!(sent, WorkerAttentionRelayOutcome::Persisted { .. }));
            assert_eq!(sent, retry);
        }
        let queue = crate::store::open_supervisor_queue_store(&cas_dir).unwrap();
        assert_eq!(queue.peek("supervisor-id", 10).unwrap().len(), 2);
        assert_eq!(enqueue_worker_attention_relay_for_episode(&cas_dir, &event, None), WorkerAttentionRelayOutcome::Pending);
        assert_eq!(queue.peek("supervisor-id", 10).unwrap().len(), 2);
        assert_ne!(attention_episode_key("a:b", "c", start), attention_episode_key("a", "b:c", start));
    }

    fn assert_recovery_class_requires_daemon(row: &cas_store::QueuedPrompt) {
        use super::super::queue_and_events::{PaneWakeState, SupervisorWakeClass, ToolCallEvidence, WakeSender};
        use crate::ui::factory::director::{AgentSummary, DirectorData};
        let now = chrono::Utc::now();
        let data = DirectorData {
            ready_tasks: vec![],
            in_progress_tasks: vec![],
            epic_tasks: vec![],
            agents: vec![AgentSummary {
                id: "supervisor-id".into(),
                name: "supervisor".into(),
                status: AgentStatus::Idle,
                registered_at: now,
                current_task: None,
                latest_activity: None,
                last_heartbeat: None,
                pending_messages: 0,
                pending_supervisor_messages: 0,
                latest_supervisor_message_at: None,
                active_lease: None,
                effort: None,
                qa_reviewer_between_rounds: false,
            }],
            activity: vec![],
            agent_id_to_name: HashMap::new(),
            changes: vec![],
            git_loaded: true,
            reminders: vec![],
            epic_closed_counts: HashMap::new(),
            start_gated_task_ids: Default::default(),
        };
        let pane = PaneWakeState {
            composer_dirty: false,
            ready_for_injection: true,
            silent_for: Some(std::time::Duration::from_secs(600)),
            tool_call: ToolCallEvidence::Idle,
        };
        let sender = FactoryDaemon::wake_sender_from_origin(row.origin.as_ref(), None);
        let decision = FactoryDaemon::supervisor_wake_decision(
            &data,
            "supervisor",
            "supervisor",
            &sender,
            &row.source,
            &row.prompt,
            pane,
            now,
        );
        assert!(
            decision.allowed,
            "emitted recovery must wake: {} ({})",
            row.prompt, decision.reason
        );
        assert_eq!(
            FactoryDaemon::supervisor_wake_class(&sender, "supervisor", &row.source, &row.prompt),
            Some(SupervisorWakeClass::Lifecycle),
        );
        for sender in [
            WakeSender::Unstamped,
            WakeSender::Unattributed,
            WakeSender::Unresolvable,
            WakeSender::Registered {
                role: AgentRole::Worker,
                name: "gold-fox".into(),
            },
        ] {
            let decision = FactoryDaemon::supervisor_wake_decision(
                &data,
                "supervisor",
                "supervisor",
                &sender,
                &row.source,
                &row.prompt,
                pane,
                now,
            );
            // An authenticated worker can send an ordinary directed message,
            // but quoting the relay cannot acquire the daemon's Lifecycle class.
            let expected = if matches!(sender, WakeSender::Registered { .. }) {
                Some(SupervisorWakeClass::DirectedMessage)
            } else { None };
            assert_eq!(
                FactoryDaemon::supervisor_wake_class(&sender, "supervisor", &row.source, &row.prompt),
                expected, "relay authority must follow the authenticated sender: {sender:?}",
            );
            assert_eq!(decision.allowed, expected.is_some(), "{sender:?}: {}", decision.reason);
        }
        for prompt in [
            format!("quoted text {}", row.prompt),
            "plain recovery instruction".into(),
        ] {
            assert!(
                !FactoryDaemon::supervisor_wake_decision(
                    &data,
                    "supervisor",
                    "supervisor",
                    &WakeSender::Daemon,
                    &row.source,
                    &prompt,
                    pane,
                    now,
                )
                .allowed,
                "free text must not wake"
            );
        }
    }

    #[test]
    fn gh_1163_delivery_stall_relays_coalesce_concurrent_messages_for_one_worker() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "worker-attention-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "worker-attention-test");
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-10T14:31:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (1190..1194)
                .map(|id| {
                    let cas_dir = &cas_dir;
                    scope.spawn(move || {
                        enqueue_worker_delivery_stalled_relay(cas_dir, "busy-codex", id, now)
                    })
                })
                .collect();
            for handle in handles {
                assert!(matches!(
                    handle.join().unwrap(),
                    WorkerAttentionRelayOutcome::Persisted { .. }
                ));
            }
        });
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        assert_eq!(
            queue.peek_all(20).unwrap().len(),
            1,
            "a burst of messages to the same worker is one incident"
        );
        assert!(matches!(
            enqueue_worker_delivery_stalled_relay(&cas_dir, "other-worker", 1200, now),
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        assert_eq!(
            queue.peek_all(20).unwrap().len(),
            2,
            "workers retain separate incidents"
        );
        // A read/ack and reopening the stores do not reset the durable window.
        for row in queue.peek_all(20).unwrap() {
            queue.ack(row.id).unwrap();
            queue.mark_processed(row.id).unwrap();
        }
        assert!(matches!(
            enqueue_worker_delivery_stalled_relay(
                &cas_dir,
                "busy-codex",
                1201,
                now + chrono::Duration::seconds(10)
            ),
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        assert_eq!(queue.peek_all(20).unwrap().len(), 0);
        assert!(matches!(
            enqueue_worker_delivery_stalled_relay(
                &cas_dir,
                "busy-codex",
                1202,
                now + chrono::Duration::minutes(5)
            ),
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        assert_eq!(
            queue.peek_all(20).unwrap().len(),
            1,
            "a later window may report a new incident"
        );
    }

    /// cas-207e: ready work waiting on an actionable-idle supervisor wakes it
    /// as a blocker (Daemon-stamped cas-blocker envelope), not as routine.
    #[test]
    fn ready_work_with_idle_workers_wakes_the_supervisor_as_a_blocker_cas_207e() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "ready-work-blocker-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "ready-work-blocker-test");
        let outcome = enqueue_worker_attention_relay(
            &cas_dir,
            &crate::ui::factory::director::DirectorEvent::SupervisorStalled {
                next_step:
                    crate::ui::factory::director::SupervisorActionableState::AssignReadyWork {
                        task_ids: vec!["cas-ready".into()],
                        idle_workers: vec!["calm-owl".into()],
                    },
                occurrence: "episode-1".into(),
                actionable_idle_secs: 600,
                operator_page_episode: None,
            },
        );
        assert!(matches!(
            outcome,
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        let rows = crate::store::open_prompt_queue_store(&cas_dir)
            .unwrap()
            .peek_all(10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].prompt.contains("cas-ready"));
        assert!(
            crate::prompt_revalidation::parse_blocker_envelope(&rows[0].prompt).is_some(),
            "{}",
            rows[0].prompt
        );
        assert_eq!(rows[0].origin, Some(cas_store::QueueOrigin::Daemon));
    }

    /// cas-7456 (GH #1302 B): a verification dispatch expired overnight with
    /// no escalation. The sweep wakes the supervisor once per expired
    /// dispatch, and leaves due-later and day-old dispatches alone.
    #[test]
    fn an_expired_verification_dispatch_wakes_the_supervisor_once_gh1302() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "dispatch-expiry-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "dispatch-expiry-test");
        let tasks = crate::store::open_task_store_local(&cas_dir).unwrap();
        for id in ["cas-b4d8", "cas-ancient", "cas-due-later"] {
            tasks.add(&cas_types::Task::new(id.into(), id.into())).unwrap();
        }
        let now = chrono::Utc::now();
        let dispatch = |task: &str, deadline| {
            cas_store::create_verification_dispatch(&cas_dir, task, "worker-id", "supervisor-id", deadline)
                .unwrap()
        };
        let expired = dispatch("cas-b4d8", now - chrono::Duration::minutes(5));
        dispatch("cas-ancient", now - chrono::Duration::hours(30));
        dispatch("cas-due-later", now + chrono::Duration::minutes(10));

        assert_eq!(escalate_expired_verification_dispatches(&cas_dir, now), 1);
        escalate_expired_verification_dispatches(&cas_dir, now);
        let rows = crate::store::open_prompt_queue_store(&cas_dir).unwrap().peek_all(10).unwrap();
        assert_eq!(rows.len(), 1, "one wake per expired dispatch, repeat sweeps included: {rows:?}");
        assert!(rows[0].prompt.contains(&expired.id) && rows[0].prompt.contains("cas-b4d8"), "{}", rows[0].prompt);
        // pin: the lifecycle-wake source makes the row a supervisor wake and puts it under the GH #894 operator watchdog
        assert!(rows[0].source.starts_with("lifecycle-wake:worker-attention:"), "{}", rows[0].source);
    }

    /// cas-b080: more expiries in the window than one sweep relays. The
    /// sweep skips dispatches it already woke the supervisor for, so newer
    /// ones are reached on the next sweep instead of starving behind the
    /// oldest twenty forever.
    #[test]
    fn expired_dispatches_past_one_sweeps_cap_are_reached_on_the_next_sweep() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "dispatch-burst-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "dispatch-burst-test");
        let tasks = crate::store::open_task_store_local(&cas_dir).unwrap();
        let now = chrono::Utc::now();
        let dispatches = (0..25)
            .map(|n| {
                let id = format!("cas-burst{n:02}");
                tasks.add(&cas_types::Task::new(id.clone(), id.clone())).unwrap();
                cas_store::create_verification_dispatch(
                    &cas_dir,
                    &id,
                    "worker-id",
                    "supervisor-id",
                    now - chrono::Duration::minutes(60 - n),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        escalate_expired_verification_dispatches(&cas_dir, now);
        escalate_expired_verification_dispatches(&cas_dir, now);
        let queue = crate::store::open_supervisor_queue_store(&cas_dir).unwrap();
        let woken = dispatches
            .iter()
            .filter(|dispatch| {
                let key = worker_attention_relay_key(
                    Some("dispatch-burst-test"),
                    "verification_dispatch_expired",
                    "supervisor",
                    &dispatch.id,
                );
                queue
                    .get_by_transition_key(&key)
                    .unwrap()
                    .is_some_and(|relay| relay.prompt_delivered_at.is_some())
            })
            .count();
        assert_eq!(woken, 25, "the newest five must not starve behind the oldest twenty");
    }

    /// cas-7456 (GH #1302 D): a supervisor-idle page flagged for the operator
    /// becomes one durable operator turn naming the waiting work, alongside
    /// the supervisor's own wake. cas-b080: the tracker asks on every wake of
    /// the silence, so later pages of the same episode (new occurrences) must
    /// not page the operator again.
    #[test]
    fn an_operator_flagged_idle_page_reaches_the_operator_once_per_episode_gh1302() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "idle-page-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "idle-page-test");
        let episode = chrono::DateTime::parse_from_rfc3339("2026-10-20T01:05:00Z").unwrap().to_utc();
        for occurrence in ["2026-10-20T03:05:00Z", "2026-10-20T03:15:00Z"] {
            let event = crate::ui::factory::director::DirectorEvent::SupervisorStalled {
                next_step: crate::ui::factory::director::SupervisorActionableState::AssignReadyWork {
                    task_ids: vec!["cas-ready".into()],
                    idle_workers: vec!["calm-owl".into()],
                },
                occurrence: occurrence.into(),
                actionable_idle_secs: 7200,
                operator_page_episode: Some(episode),
            };
            enqueue_worker_attention_relay(&cas_dir, &event);
        }
        let rows = crate::store::open_prompt_queue_store(&cas_dir).unwrap().peek_all(10).unwrap();
        let operator = rows.iter().filter(|row| row.target == "operator").collect::<Vec<_>>();
        assert_eq!(operator.len(), 1, "{rows:?}");
        assert!(operator[0].prompt.contains("cas-ready") && operator[0].prompt.contains("calm-owl"), "{}", operator[0].prompt);
        assert!(rows.iter().any(|row| row.target != "operator"), "the supervisor is still woken: {rows:?}");
    }

    #[test]
    fn taskless_idle_and_escalated_stall_use_durable_wake_relay() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "worker-attention-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "worker-attention-test");

        let _ = enqueue_worker_attention_relay(
            &cas_dir,
            &crate::ui::factory::director::DirectorEvent::WorkerIdle {
                worker: "calm-owl".to_string(),
                active_task: None,
            },
        );
        let _ = enqueue_worker_delivery_stalled_relay(&cas_dir, "silent-codex", 42, chrono::Utc::now());
        let _ = enqueue_worker_unavailable_relay(&cas_dir, "limited-codex", "episode-1");
        let replay = enqueue_worker_unavailable_relay(&cas_dir, "limited-codex", "episode-1");
        assert!(matches!(
            replay,
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        let _ = enqueue_worker_attention_relay(
            &cas_dir,
            &crate::ui::factory::director::DirectorEvent::WorkerStalled {
                worker: "steady-otter".to_string(),
                task_id: "cas-d4ae".to_string(),
                elapsed_secs: 300,
                escalate: true,
            },
        );

        let _ = enqueue_usage_limit_relay(&cas_dir, "limited", "episode-2",
            "Worker limited, task cas-held: usage limit; waiting for reset. No restart queued.");
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 5, "a persisted occurrence replays idempotently");
        // pin: find the limit notice by its worker subject before checking recovery guidance.
        let limited = rows.iter().find(|row| row.prompt.contains("Worker limited,")).unwrap();
        // pin: a stopped worker must retain supervisor recovery guidance.
        assert!(limited.prompt.contains("reassign or recover"), "{}", limited.prompt);
        assert!(rows.iter().all(|row| {
            row.target == "supervisor"
                && crate::mcp::tools::core::task::lifecycle::supervisor_push::is_lifecycle_wake_source(&row.source)
                && crate::prompt_revalidation::is_supervisor_wake_envelope(&row.prompt)
        }));
        assert!(
            rows.iter()
                .any(|row| row.prompt.contains("kind=\"worker_idle\""))
        );
        assert!(rows.iter().any(|row| {
            row.prompt.contains("kind=\"worker_stalled\"") && row.prompt.contains("cas-d4ae")
        }));
        assert!(rows.iter().any(|row| {
            row.prompt.contains("kind=\"worker_delivery_stalled\"")
                && row.prompt.contains("silent-codex")
                && row.prompt.contains("normal message 42")
        }));
        assert!(rows.iter().any(|row| {
            row.prompt.contains("kind=\"worker_unavailable\"")
                && row.prompt.contains("limited-codex")
                && row.prompt.contains("terminal unavailable state")
        }));
    }

    /// cas-2ffe (GH #915): three simultaneous harness exits reach the
    /// supervisor as ONE incident naming every worker and exit status, and a
    /// replay of the same group stays one wake.
    #[test]
    fn simultaneous_worker_deaths_wake_the_supervisor_once_cas_2ffe() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "correlated-deaths-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "correlated-deaths-test");
        let t0 = chrono::Utc::now();
        let group = ["codex-a", "codex-b", "codex-c"]
            .iter()
            .enumerate()
            .map(|(index, worker)| super::super::queue_and_events::ObservedWorkerExit {
                at: t0 + chrono::Duration::milliseconds(600 * index as i64),
                worker: (*worker).to_string(),
                status: "exited with code 0".to_string(),
                death: None,
                recording: false,
            })
            .collect::<Vec<_>>();

        let first = enqueue_correlated_worker_deaths_relay(&cas_dir, &group);
        let replay = enqueue_correlated_worker_deaths_relay(&cas_dir, &group);
        assert!(matches!(first, WorkerAttentionRelayOutcome::Persisted { .. }));
        assert_eq!(first, replay);

        let rows = crate::store::open_prompt_queue_store(&cas_dir)
            .unwrap()
            .peek_all(10)
            .unwrap();
        assert_eq!(rows.len(), 1, "one incident, one wake");
        let prompt = &rows[0].prompt;
        assert!(crate::prompt_revalidation::is_supervisor_wake_envelope(prompt), "{prompt}");
        assert!(prompt.contains("kind=\"workers_died_together\""), "{prompt}");
        assert!(prompt.contains("3 worker harnesses exited within 1200 ms"), "{prompt}");
        for worker in ["codex-a", "codex-b", "codex-c"] {
            assert!(prompt.contains(&format!("{worker} (exited with code 0)")), "{prompt}");
        }
    }

    /// cas-26c8 (spec-6194 P2.6, ct2_p2_died_together_folds): workers that die
    /// unexpectedly together reach the supervisor as ONE Critical relay with
    /// one entry per worker (tasks and cleaned tail), not N worker_died relays
    /// plus a workers_died_together relay. A lone death is unchanged: one
    /// worker_died relay.
    #[tokio::test(flavor = "current_thread")]
    async fn ct2_p2_died_together_folds() {
        use crate::mcp::tools::service::orphan_recovery::recover_worker_deferring_relay;
        use super::super::queue_and_events::{DeferredWorkerDeath, ObservedWorkerExit};
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "provision-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "provision-test");
        let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let t0 = chrono::Utc::now() - chrono::Duration::seconds(30);
        let died_at = |name: &str, at: chrono::DateTime<chrono::Utc>| {
            let mut worker = Agent::new(format!("{name}-id"), name.to_string());
            worker.role = AgentRole::Worker;
            worker.status = AgentStatus::Active;
            worker.factory_session = Some("provision-test".to_string());
            agents.register(&worker).unwrap();
            let exit = crate::prompt_revalidation::WorkerExitCause {
                exit_code: Some(0),
                exit_signal: None,
                output_tail: Some(format!("{name} last line")),
            };
            let summary = recover_worker_deferring_relay(
                &cas_dir,
                agents.as_ref(),
                &worker,
                &[],
                "exited with code 0",
                Some(&exit),
            )
            .expect("an unexpected death is recorded with its relay held");
            ObservedWorkerExit {
                at,
                worker: name.to_string(),
                status: exit.status(),
                death: Some(DeferredWorkerDeath {
                    agent: worker,
                    summary,
                    reason: "exited with code 0".to_string(),
                    exit,
                }),
                recording: false,
            }
        };
        let died = |name: &str, offset_ms: i64| died_at(name, t0 + chrono::Duration::milliseconds(offset_ms));
        let notices = || {
            crate::store::open_supervisor_queue_store(&cas_dir)
                .unwrap()
                .list_pending("supervisor-id")
                .unwrap()
        };
        let prompt_rows = || {
            crate::store::open_prompt_queue_store(&cas_dir)
                .unwrap()
                .peek_all(50)
                .unwrap()
        };

        daemon.recent_worker_exits = vec![died("codex-a", 0), died("codex-b", 700), died("codex-c", 1_200)];
        // Review M: each held death is durable at once; only its prompt waits.
        assert_eq!(
            notices().iter().filter(|row| row.event_type == "worker_died").count(),
            3,
            "the deaths are recorded before the window closes"
        );
        // pin: review M contract: a held death has its durable row but no prompt yet.
        assert!(prompt_rows().is_empty(), "no prompt until the window closes");
        daemon.last_death_sweep = None;
        daemon.relay_correlated_worker_deaths();
        let rows = notices();
        let kinds: Vec<&str> = rows.iter().map(|row| row.event_type.as_str()).collect();
        assert_eq!(kinds, ["workers_died_together"], "one folded relay, no per-worker relays");
        assert_eq!(rows[0].priority, cas_store::NotificationPriority::Critical);
        let prompts = prompt_rows();
        // pin: the AC is exactly one supervisor wake for the whole group.
        assert_eq!(prompts.len(), 1, "one wake: {prompts:?}");
        let body = &prompts[0].prompt;
        for worker in ["codex-a", "codex-b", "codex-c"] {
            let entry = format!("- {worker} (exited with code 0)");
            let tail = format!("> {worker} last line");
            // pin: one entry per worker naming its exit status (the folded per-worker relay).
            assert!(body.contains(&entry), "{body}");
            // pin: each entry quotes that worker's cleaned output tail.
            assert!(body.contains(&tail), "each entry carries the worker's cleaned tail: {body}");
        }
        assert!(daemon.recent_worker_exits.is_empty());

        daemon.recent_worker_exits = vec![died("codex-d", 0)];
        daemon.last_death_sweep = None;
        daemon.relay_correlated_worker_deaths();
        let rows = notices();
        let lone: Vec<_> = rows.iter().filter(|row| row.event_type == "worker_died").collect();
        assert_eq!(lone.len(), 1, "a lone death relays as before: {rows:?}");
        // pin: review L: the lone death's PROMPT reaches the supervisor, not only its row.
        assert!(prompt_rows().iter().any(|row| row.prompt.contains("codex-d")), "lone prompt");
        assert!(lone[0].payload.contains("codex-d"), "{}", lone[0].payload);
        assert_eq!(lone[0].priority, cas_store::NotificationPriority::Critical);
        assert_eq!(
            rows.iter().filter(|row| row.event_type == "workers_died_together").count(),
            1,
            "a lone death adds no group relay"
        );

        // Review M: a daemon that stops or restarts inside the window loses
        // what it held in memory. While the window is open nothing is sent;
        // once nothing holds the deaths, the sweep injects their relays.
        let before = prompt_rows().len();
        let now = chrono::Utc::now();
        daemon.recent_worker_exits = vec![died_at("codex-e", now), died_at("codex-f", now)];
        daemon.last_death_sweep = None;
        daemon.relay_correlated_worker_deaths();
        // pin: the sweep must skip deaths the live daemon still holds.
        assert_eq!(prompt_rows().len(), before, "held deaths wait for their window");
        daemon.recent_worker_exits.clear(); // the restarted daemon holds nothing
        daemon.last_death_sweep = None;
        daemon.relay_correlated_worker_deaths();
        let injected = prompt_rows();
        assert_eq!(injected.len(), before + 2, "both deaths still reach the supervisor");
        for worker in ["codex-e", "codex-f"] {
            assert!(
                injected.iter().any(|row| row.prompt.contains(worker)),
                "{worker}: {injected:?}"
            );
        }
        daemon.last_death_sweep = None;
        daemon.relay_correlated_worker_deaths();
        // pin: a second sweep must not inject the same deaths again.
        assert_eq!(prompt_rows().len(), before + 2, "the sweep injects each death once");
    }

    /// cas-26c8 review M1 crash test: the daemon stops after the folded
    /// relay's durable row is written (members stamped in that transaction)
    /// but before its prompt. The restarted sweep re-drives the folded prompt
    /// once and injects none of the folded per-worker relays. Review H1: a
    /// second factory sharing the database is left alone.
    #[tokio::test(flavor = "current_thread")]
    async fn folded_relay_survives_a_crash_before_its_prompt_cas_26c8() {
        use crate::mcp::tools::service::orphan_recovery::{
            death_incident_key, recover_worker_deferring_relay,
        };
        use cas_store::SupervisorQueueStore;
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "provision-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "provision-test");
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let mut other = Agent::new("other-supervisor-id".to_string(), "other-supervisor".to_string());
        other.role = AgentRole::Supervisor;
        other.status = AgentStatus::Active;
        other.factory_session = Some("other-factory".to_string());
        agents.register(&other).unwrap();
        let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
        let record = |name: &str, session: &str| {
            let mut worker = Agent::new(format!("{name}-id"), name.to_string());
            worker.role = AgentRole::Worker;
            worker.status = AgentStatus::Active;
            worker.factory_session = Some(session.to_string());
            agents.register(&worker).unwrap();
            recover_worker_deferring_relay(&cas_dir, agents.as_ref(), &worker, &[], "exited", None)
                .expect("recorded");
            death_incident_key(&worker)
        };
        let incidents = vec![record("codex-g", "provision-test"), record("codex-h", "provision-test")];
        record("other-w", "other-factory");

        // What the folded relay writes before its prompt, then the crash.
        let occurrence = "codex-g:1";
        let payload = serde_json::json!({
            "kind": "workers_died_together",
            "worker": "codex-g",
            "detail": "2 worker harnesses exited together: codex-g; codex-h",
            "factory_session": "provision-test",
            "occurrence": occurrence,
            "folded_incidents": incidents,
        });
        let queue = crate::store::open_supervisor_queue_store(&cas_dir).unwrap();
        queue
            .notify_idempotent_folding(
                "supervisor-id",
                "workers_died_together",
                &payload.to_string(),
                cas_store::NotificationPriority::Critical,
                &format!("worker-attention:provision-test:workers_died_together:codex-g:{occurrence}"),
                "worker_died",
                &incidents,
            )
            .unwrap();

        daemon.last_death_sweep = None;
        // cas-ff42 L4: another factory's crashed fold, in the same database.
        let other_payload = serde_json::json!({
            "kind": "workers_died_together",
            "worker": "other-w",
            "detail": "2 worker harnesses exited together: other-w; other-x",
            "factory_session": "other-factory",
            "occurrence": "other-w:1",
            "folded_incidents": [],
        });
        queue
            .notify_idempotent(
                "other-supervisor-id",
                "workers_died_together",
                &other_payload.to_string(),
                cas_store::NotificationPriority::Critical,
                "worker-attention:other-factory:workers_died_together:other-w:other-w:1",
            )
            .unwrap();

        daemon.last_death_sweep = None;
        daemon.relay_correlated_worker_deaths(); // the restarted daemon holds nothing
        let prompts = crate::store::open_prompt_queue_store(&cas_dir)
            .unwrap()
            .peek_all(20)
            .unwrap();
        let folded = prompts
            .iter()
            // pin: the folded relay is recognised by its worker-attention kind attribute.
            .filter(|row| row.prompt.contains("kind=\"workers_died_together\""))
            .count();
        // pin: exactly the folded prompt is re-driven after the crash.
        assert_eq!(folded, 1, "{prompts:?}");
        let per_worker = prompts
            .iter()
            .filter(|row| row.prompt.contains("worker-died") || row.summary.as_deref().is_some_and(|s| s.starts_with("worker died")))
            .count();
        assert_eq!(per_worker, 0, "folded members are never injected: {prompts:?}");
        assert!(
            !prompts.iter().any(|row| row.prompt.contains("other-w")),
            "another factory's held death and crashed fold are not swept: {prompts:?}"
        );
        daemon.last_death_sweep = None;
        daemon.relay_correlated_worker_deaths();
        let again = crate::store::open_prompt_queue_store(&cas_dir).unwrap().peek_all(20).unwrap();
        // pin: review M1: a second sweep must not re-drive the fold again.
        assert_eq!(again.len(), prompts.len(), "the re-drive happens once");
    }

    /// cas-ff42 L3: a recorded death whose prompt never went out is given up
    /// on once it ages out of the sweep, with one warning and no late alarm.
    #[tokio::test(flavor = "current_thread")]
    async fn the_death_sweep_reports_giving_up_once_cas_ff42() {
        use cas_store::SupervisorQueueStore;
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "provision-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "provision-test");
        let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
        let queue = crate::store::open_supervisor_queue_store(&cas_dir).unwrap();
        let payload = serde_json::json!({
            "worker_id": "stale-w-id",
            "worker_name": "stale-w",
            "factory_session": "provision-test",
        });
        let cas_store::NotifyIdempotentResult::Created(id) = queue
            .notify_idempotent(
                "supervisor-id",
                "worker_died",
                &payload.to_string(),
                cas_store::NotificationPriority::Critical,
                "worker_died:stale-w-id:1:supervisor-id",
            )
            .unwrap()
        else {
            unreachable!()
        };
        let aged = (chrono::Utc::now() - chrono::Duration::minutes(40)).to_rfc3339();
        rusqlite::Connection::open(cas_dir.join("cas.db"))
            .unwrap()
            .execute(
                "UPDATE supervisor_queue SET created_at = ?1 WHERE id = ?2",
                rusqlite::params![aged, id],
            )
            .unwrap();

        for _ in 0..2 {
            daemon.last_death_sweep = None;
            daemon.relay_correlated_worker_deaths();
        }
        assert!(daemon.relay_jobs.death_sweep_given_up.contains(&id), "the give-up is reported");
        assert_eq!(daemon.relay_jobs.death_sweep_given_up.len(), 1, "reported once");
        let prompts = crate::store::open_prompt_queue_store(&cas_dir).unwrap().peek_all(10).unwrap();
        // pin: cas-ff42 L3: giving up means no late per-worker alarm.
        assert!(prompts.is_empty(), "an aged-out death is not sent late: {prompts:?}");
    }

    /// cas-26c8 review L: pin the folded relay's L2 signal on the normal path.
    #[test]
    fn folded_relay_reports_its_prompt_enqueued_cas_26c8() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "folded-signal",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "folded-signal");
        let t0 = chrono::Utc::now();
        let group = ["codex-a", "codex-b"]
            .iter()
            .map(|worker| super::super::queue_and_events::ObservedWorkerExit {
                at: t0,
                worker: (*worker).to_string(),
                status: "exited with code 0".to_string(),
                death: None,
                recording: false,
            })
            .collect::<Vec<_>>();
        let attempt = enqueue_folded_worker_deaths_relay(&cas_dir, &group);
        assert!(
            matches!(attempt.outcome, WorkerAttentionRelayOutcome::Persisted { .. }),
            "{attempt:?}"
        );
        // pin: review L2 contract: a persisted fold reports prompt_enqueued=true.
        assert!(attempt.prompt_enqueued, "a persisted fold reports its prompt enqueued");
        // pin: cas-ff42 L1: a persisted fold reports its row committed.
        assert!(attempt.recorded, "a persisted fold reports its row recorded");
    }

    /// cas-26c8 review L1: every closed group is listed on its own, so a group
    /// whose folded relay is retried never holds back a later lone death.
    #[test]
    fn a_stuck_death_group_does_not_block_later_deaths_cas_26c8() {
        use super::super::queue_and_events::{
            ObservedWorkerExit, WORKER_DEATH_CORRELATION_WINDOW, closed_death_groups,
            death_group_key,
        };
        let t0 = chrono::Utc::now() - chrono::Duration::seconds(60);
        let exit = |worker: &str, offset_ms: i64| ObservedWorkerExit {
            at: t0 + chrono::Duration::milliseconds(offset_ms),
            worker: worker.to_string(),
            status: "exited with code 0".to_string(),
            death: None,
            recording: false,
        };
        let exits = vec![exit("stuck-a", 0), exit("stuck-b", 500), exit("lone-c", 20_000)];
        let groups = closed_death_groups(&exits, chrono::Utc::now(), WORKER_DEATH_CORRELATION_WINDOW);
        let names: Vec<Vec<&str>> = groups
            .iter()
            .map(|group| group.iter().map(|exit| exit.worker.as_str()).collect())
            .collect();
        assert_eq!(names, [vec!["stuck-a", "stuck-b"], vec!["lone-c"]]);
        assert_ne!(
            death_group_key(&groups[0]),
            death_group_key(&groups[1]),
            "each group has its own write key, so neither waits on the other"
        );
    }

    /// cas-26c8 review L2 / cas-ff42 L1 and L3: a committed fold is done once
    /// its prompt is queued or the death sweep re-drives its recipient. A
    /// fold to a recipient the sweep never re-drives (`operator`, or one
    /// outside the session) keeps retrying, then gives up with a warning. The
    /// per-worker fallback only covers a fold that never committed, so no
    /// path sends twice.
    #[test]
    fn folded_group_step_covers_every_recovery_path_cas_ff42() {
        use super::super::queue_and_events::{FoldedGroupStep, folded_group_step};
        let young = chrono::Duration::seconds(30);
        let old = chrono::Duration::minutes(11);
        // (recorded, prompt queued, sweep re-drives the recipient, age)
        assert_eq!(folded_group_step(true, true, false, young), FoldedGroupStep::Fold);
        assert_eq!(folded_group_step(true, false, true, young), FoldedGroupStep::Fold, "the sweep owns it");
        assert_eq!(folded_group_step(true, true, false, old), FoldedGroupStep::Fold, "no double send");
        assert_eq!(
            folded_group_step(true, false, false, young),
            FoldedGroupStep::Retry,
            "an operator fold whose prompt failed is retried, not marked done"
        );
        assert_eq!(folded_group_step(true, false, false, old), FoldedGroupStep::GiveUp);
        assert_eq!(folded_group_step(false, false, false, young), FoldedGroupStep::Retry);
        assert_eq!(folded_group_step(false, false, false, old), FoldedGroupStep::FallBack);
    }

    /// cas-ff42 review: the name fallback never marks a same-name respawn
    /// that registered after the exit as the dead worker.
    #[test]
    fn a_death_record_never_takes_a_later_same_name_respawn_cas_ff42() {
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let exit_at = chrono::Utc::now() - chrono::Duration::seconds(30);
        for (id, heartbeat) in [
            ("old-id", exit_at - chrono::Duration::seconds(5)),
            ("respawn-id", exit_at + chrono::Duration::seconds(10)),
        ] {
            let mut worker = Agent::new(id.to_string(), "codex-r".to_string());
            worker.role = AgentRole::Worker;
            worker.status = AgentStatus::Active;
            worker.factory_session = Some("provision-test".to_string());
            agents.register(&worker).unwrap();
            let mut stored = agents.get(id).unwrap();
            stored.last_heartbeat = heartbeat;
            agents.update(&stored).unwrap();
        }
        let death = super::super::queue_and_events::record_worker_death_io(
            &cas_dir,
            "provision-test",
            "codex-r",
            None,
            "exited with code 1",
            None,
            crate::mcp::tools::service::orphan_recovery::DeathRelay::Deferred,
            Some(exit_at),
        );
        // A live same-name successor means no death is reported (GH #924);
        // whatever is reported, it is never the respawn's.
        assert!(death.is_none_or(|death| death.agent.id == "old-id"));
        assert_eq!(agents.get("respawn-id").unwrap().status, AgentStatus::Active, "the respawn is untouched");
        assert_ne!(agents.get("old-id").unwrap().status, AgentStatus::Active, "the dead row is stale");
    }

    /// cas-ff42 review L2: a live Director that also gets worker deaths keeps
    /// a prompt of its own: the fold's per-recipient copy is prompted to it
    /// by the death sweep, and its per-worker rows are folded away.
    #[tokio::test(flavor = "current_thread")]
    async fn a_live_director_is_prompted_with_its_fold_copy_cas_ff42() {
        use crate::mcp::tools::service::orphan_recovery::recover_worker_deferring_relay;
        use super::super::queue_and_events::{DeferredWorkerDeath, ObservedWorkerExit};
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "provision-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "provision-test");
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let mut director = Agent::new("director-id".to_string(), "director".to_string());
        director.role = AgentRole::Director;
        director.status = AgentStatus::Active;
        director.factory_session = Some("provision-test".to_string());
        agents.register(&director).unwrap();
        let mut daemon = super::super::provisioning_tests::daemon(&cas_dir);
        let t0 = chrono::Utc::now() - chrono::Duration::seconds(30);
        let died = |name: &str, offset_ms: i64| {
            let mut worker = Agent::new(format!("{name}-id"), name.to_string());
            worker.role = AgentRole::Worker;
            worker.status = AgentStatus::Active;
            worker.factory_session = Some("provision-test".to_string());
            agents.register(&worker).unwrap();
            let summary =
                recover_worker_deferring_relay(&cas_dir, agents.as_ref(), &worker, &[], "exited", None)
                    .expect("recorded");
            ObservedWorkerExit {
                at: t0 + chrono::Duration::milliseconds(offset_ms),
                worker: name.to_string(),
                status: "exited with code 0".to_string(),
                death: Some(DeferredWorkerDeath {
                    agent: worker,
                    summary,
                    reason: "exited".to_string(),
                    exit: Default::default(),
                }),
                recording: false,
            }
        };
        daemon.recent_worker_exits = vec![died("codex-p", 0), died("codex-q", 400)];
        for _ in 0..2 {
            daemon.last_death_sweep = None;
            daemon.relay_correlated_worker_deaths();
        }
        let prompts = crate::store::open_prompt_queue_store(&cas_dir).unwrap().peek_all(20).unwrap();
        let folded_to = |target: &str| {
            prompts
                .iter()
                // pin: the folded relay is recognised by its worker-attention kind attribute.
                .filter(|row| row.target == target && row.prompt.contains("kind=\"workers_died_together\""))
                .count()
        };
        // pin: the supervisor and the Director each get exactly one folded prompt.
        assert_eq!(folded_to("supervisor"), 1, "{prompts:?}");
        assert_eq!(folded_to("director"), 1, "the Director is prompted with its copy: {prompts:?}");
        let pending = crate::store::open_supervisor_queue_store(&cas_dir)
            .unwrap()
            .list_pending("director-id")
            .unwrap();
        assert!(
            pending.iter().all(|row| row.event_type != "worker_died"),
            "the Director's per-worker rows are folded away: {pending:?}"
        );
    }


    /// cas-4143: a parked teammate permission request becomes one durable
    /// supervisor wake that names the worker, command and age and carries the
    /// approve/deny commands. Repeat scans replay it idempotently.
    #[test]
    fn a_pending_leader_approval_wakes_the_supervisor_once_cas_4143() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "approval-pending-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "approval-pending-test");
        let request = crate::factory_permission_relay::WorkerPermissionRequest {
            request_id: "perm-1790189281659-ph1zl7g".to_string(),
            worker: "solid-falcon-70".to_string(),
            tool_name: "Bash".to_string(),
            tool_use_id: Some("toolu_01Heredoc".to_string()),
            command_excerpt: "cd hub-web && python3 - <<'EOF'".to_string(),
            age_secs: 1680,
        };

        let first = enqueue_worker_approval_pending_relay(&cas_dir, &request);
        let replay = enqueue_worker_approval_pending_relay(&cas_dir, &request);
        assert!(matches!(first, WorkerAttentionRelayOutcome::Persisted { .. }));
        assert_eq!(first, replay, "one wake per request id");

        let rows = crate::store::open_prompt_queue_store(&cas_dir)
            .unwrap()
            .peek_all(10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        let prompt = &rows[0].prompt;
        assert_eq!(rows[0].target, "supervisor");
        assert!(crate::prompt_revalidation::is_supervisor_wake_envelope(prompt), "{prompt}");
        assert!(prompt.contains("kind=\"worker_approval_pending\""), "{prompt}");
        assert!(prompt.contains("solid-falcon-70") && prompt.contains("1680s"), "{prompt}");
        assert!(prompt.contains("cd hub-web && python3 - <<'EOF'"), "{prompt}");
        assert!(
            prompt.contains("cas factory approve solid-falcon-70 --request perm-1790189281659-ph1zl7g")
                && prompt.contains("cas factory deny solid-falcon-70"),
            "{prompt}"
        );
    }

    #[test]
    fn lifecycle_recovery_stalled_close_uses_registered_recipient_harness() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("CAS_FACTORY_SESSION", "recovery-matrix"),
            ("CAS_FACTORY_SUPERVISOR_CLI", "opencode"),
            ("CAS_FACTORY_WORKER_CLI", "codex"),
        ]);
        for (harness, prefix) in [
            ("claude", "mcp__cas__"),
            ("codex", "mcp__cs__"),
            ("grok", "cas__"),
            ("opencode", "cas_"),
        ] {
            let temp = tempfile::TempDir::new().unwrap();
            let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
            register_supervisor(&cas_dir, "recovery-matrix");
            let agents = crate::store::open_agent_store(&cas_dir).unwrap();
            let mut supervisor = agents.get("supervisor-id").unwrap();
            supervisor
                .metadata
                .insert("supervisor_cli".into(), harness.into());
            agents.update(&supervisor).unwrap();
            let event = crate::ui::factory::director::DirectorEvent::SupervisorStalled {
                next_step:
                    crate::ui::factory::director::SupervisorActionableState::MergeCloseBlocked {
                        tasks: vec![crate::ui::factory::director::MergedCloseBlockedTask {
                            task_id: "cas-merged".into(),
                            factory_branch: "factory/gold-fox".into(),
                            anchor: "anchor-sha".into(),
                            target_branch: "epic/cas-epic".into(),
                            target_tip: "target-tip".into(),
                            close_rejection: "gate said \"retry\"\nthen inspect".into(),
                        }],
                    },
                occurrence: "episode-1".into(),
                actionable_idle_secs: 600,
                operator_page_episode: None,
            };
            if harness == "codex" {
                // Replay a durable-only notification after the registered
                // receiver changes harness; the unsent action must use Codex.
                let crate::ui::factory::director::DirectorEvent::SupervisorStalled {
                    next_step,
                    ..
                } = &event
                else {
                    unreachable!()
                };
                let key = format!(
                    "worker-attention:recovery-matrix:merged_close_blocked:{}",
                    next_step.merged_close_blocked_relay_key().unwrap()
                );
                supervisor
                    .metadata
                    .insert("supervisor_cli".into(), "claude".into());
                agents.update(&supervisor).unwrap();
                let queue = crate::store::open_supervisor_queue_store(&cas_dir).unwrap();
                queue
                    .notify_idempotent(
                        "supervisor-id",
                        "merged_close_blocked",
                        &serde_json::json!({ "detail": event.description() }).to_string(),
                        cas_store::NotificationPriority::High,
                        &key,
                    )
                    .unwrap();
                supervisor
                    .metadata
                    .insert("supervisor_cli".into(), harness.into());
                agents.update(&supervisor).unwrap();
            }
            assert!(matches!(
                enqueue_worker_attention_relay(&cas_dir, &event),
                WorkerAttentionRelayOutcome::Persisted { .. }
            ));
            assert!(matches!(
                enqueue_worker_attention_relay(&cas_dir, &event),
                WorkerAttentionRelayOutcome::Persisted { .. }
            ));
            let rows = crate::store::open_prompt_queue_store(&cas_dir)
                .unwrap()
                .peek_all(10)
                .unwrap();
            assert_eq!(rows.len(), 1, "replay remains idempotent");
            let row = &rows[0];
            assert!(
                row.prompt
                    .contains(&format!("{prefix}task action=close id=cas-merged")),
                "{harness}: {}",
                row.prompt
            );
            assert!(
                row.prompt
                    .contains(&format!("{prefix}factory action=worker_status")),
                "{harness}: {}",
                row.prompt
            );
            for foreign in ["mcp__cas__task", "mcp__cs__task", "cas__task", "cas_task"] {
                if foreign != format!("{prefix}task") {
                    // Match the command boundary: Claude's prefix contains Grok's as a suffix.
                    assert!(
                        !row.prompt.contains(&format!("`{foreign} ")),
                        "{harness}: {}",
                        row.prompt
                    );
                }
            }
            assert!(
                row.prompt.contains("gate said \"retry\"\nthen inspect"),
                "rejection context stays verbatim"
            );
            assert!(
                row.prompt.contains(&format!(
                    "{prefix}task action=close id=cas-merged supervisor_override=true reason=\"merged delivery verified\""
                )),
                "audit reason must remain a valid argument: {}",
                row.prompt
            );
            assert_eq!(row.origin, Some(cas_store::QueueOrigin::Daemon));
            assert!(crate::prompt_revalidation::is_supervisor_wake_envelope(
                &row.prompt
            ));
            assert_recovery_class_requires_daemon(row);
            let notifications = crate::store::open_supervisor_queue_store(&cas_dir)
                .unwrap()
                .peek("supervisor-id", 10)
                .unwrap();
            assert_eq!(notifications.len(), 1);
            assert!(
                !notifications[0].payload.contains("mcp__"),
                "durable facts must remain neutral"
            );
            let facts = event.to_json().to_string();
            assert!(
                !facts.contains("mcp__"),
                "stored event facts must be harness independent: {facts}"
            );
        }
    }

    #[test]
    fn lifecycle_recovery_missing_and_shutdown_recipient_route_to_operator() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[
            ("CAS_FACTORY_SESSION", "recovery-unknown"),
            ("CAS_FACTORY_SUPERVISOR_CLI", "codex"),
        ]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        let event = crate::ui::factory::director::DirectorEvent::WorkerIdle {
            worker: "gold-fox".into(),
            active_task: None,
        };
        assert!(matches!(
            enqueue_worker_attention_relay(&cas_dir, &event),
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        let initial = crate::store::open_prompt_queue_store(&cas_dir)
            .unwrap()
            .peek_all(10)
            .unwrap();
        assert_eq!(initial.len(), 1);
        assert_eq!(initial[0].target, "operator");
        assert_eq!(
            initial[0].factory_session.as_deref(),
            Some("recovery-unknown")
        );
        register_supervisor(&cas_dir, "recovery-unknown");
        let agents = crate::store::open_agent_store(&cas_dir).unwrap();
        let mut supervisor = agents.get("supervisor-id").unwrap();
        supervisor.status = AgentStatus::Shutdown;
        supervisor
            .metadata
            .insert("supervisor_cli".into(), "unknown".into());
        agents.update(&supervisor).unwrap();
        // A newly registered but shutdown row cannot capture the same event;
        // the operator fallback is idempotent and remains harness-neutral.
        assert!(matches!(
            enqueue_worker_attention_relay(&cas_dir, &event),
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        let rows = crate::store::open_prompt_queue_store(&cas_dir)
            .unwrap()
            .peek_all(10)
            .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, "operator");
        let payload: crate::ui::factory::OperatorReplyPayload =
            serde_json::from_str(&rows[0].prompt).unwrap();
        // pin: operator fallback must provide the harness-neutral recovery command.
        assert!(payload.message.contains("`factory action=worker_status`"));
        assert!(!rows[0].prompt.contains("mcp__"));
        assert_eq!(rows[0].origin, Some(cas_store::QueueOrigin::Daemon));
    }

    #[test]
    fn merged_close_blocked_supervisor_stall_relays_once_without_repeat_merge_demand() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "merged-close-blocked-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "merged-close-blocked-test");
        let next_step = crate::ui::factory::director::SupervisorActionableState::MergeCloseBlocked {
            tasks: vec![crate::ui::factory::director::MergedCloseBlockedTask {
                task_id: "cas-merged".to_string(),
                factory_branch: "factory/gold-fox".to_string(),
                anchor: "anchor-sha".to_string(),
                target_branch: "epic/cas-epic".to_string(),
                target_tip: "target-tip".to_string(),
                close_rejection: "ZERO-COMMIT after merged delivery".to_string(),
            }],
        };
        let first = enqueue_worker_attention_relay(
            &cas_dir,
            &crate::ui::factory::director::DirectorEvent::SupervisorStalled {
                next_step: next_step.clone(),
                occurrence: "2026-09-11T02:40:00Z".to_string(),
                actionable_idle_secs: 600,
                operator_page_episode: None,
            },
        );
        let replay = enqueue_worker_attention_relay(
            &cas_dir,
            &crate::ui::factory::director::DirectorEvent::SupervisorStalled {
                next_step,
                occurrence: "2026-09-11T02:50:00Z".to_string(),
                actionable_idle_secs: 1200,
                operator_page_episode: None,
            },
        );
        assert!(matches!(
            first,
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        assert!(matches!(
            replay,
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));

        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 1, "the merged close block is stable-deduped");
        assert!(rows[0].prompt.contains("kind=\"merged_close_blocked\""));
        assert!(rows[0]
            .prompt
            .contains("ZERO-COMMIT after merged delivery"));
        assert!(rows[0]
            .prompt
            .contains("task action=close id=cas-merged"));
        assert!(!rows[0]
            .prompt
            .contains("merge the ready delivery branch(es)"));
    }

    #[test]
    fn cas_f439_close_notices_dedupe_each_delivery_across_queue_changes() {
        use crate::ui::factory::director::{DirectorEvent, MergedCloseBlockedTask, SupervisorActionableState};
        let mut env = crate::test_support::TestEnvGuard::temp_home();
        env.set("CAS_FACTORY_SESSION", "contained-close-test");
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "contained-close-test");
        let mut delivery = MergedCloseBlockedTask {
            task_id: "cas-first".into(), factory_branch: "factory/worker-cas-first".into(),
            anchor: "first-anchor".into(), target_branch: "epic/x".into(), target_tip: "target-a".into(),
            close_rejection: "MERGE REQUIRED".into(),
        };
        let event = |state, occurrence: &str| DirectorEvent::SupervisorStalled {
            next_step: state, occurrence: occurrence.into(), actionable_idle_secs: 600,
            operator_page_episode: None,
        };
        assert!(matches!(enqueue_worker_attention_relay(&cas_dir, &event(
            SupervisorActionableState::MergeCloseBlocked { tasks: vec![delivery.clone()] }, "first")),
            WorkerAttentionRelayOutcome::Persisted { .. }));
        delivery.target_tip = "target-b".into();
        let mut second = delivery.clone();
        second.task_id = "cas-second".into();
        second.anchor = "second-anchor".into();
        assert!(matches!(enqueue_worker_attention_relay(&cas_dir, &event(
            SupervisorActionableState::MergeCloseBlocked { tasks: vec![delivery, second] }, "later")),
            WorkerAttentionRelayOutcome::Persisted { .. }));
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 2, "each contained delivery gets one notice, even as the queue changes");
        for row in rows {
            // pin: the supervisor's delivered instruction must request close, not another merge (GH #1225).
            assert!(row.prompt.contains("awaiting worker close"));
            // pin: a contained delivery must never receive the executable merge-now instruction (GH #1225).
            assert!(!row.prompt.contains("merge the ready delivery branch(es)"));
        }
    }

    /// cas-d9a8: the fail-safe for supervisor traffic that is not wake-shaped.
    /// A blocker or verification handoff stays inbox-only — waking on those
    /// would mean waking on a keyword — so CAS reports the unread COUNT
    /// instead, on the one channel that does reach an idle pane.
    #[test]
    fn the_supervisor_unread_relay_is_wake_shaped_and_fires_once_per_backlog_head() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "supervisor-unread-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "supervisor-unread-test");

        let first = enqueue_supervisor_unread_relay(&cas_dir, "daring-marten-11", 24370, 3);
        assert!(matches!(first, WorkerAttentionRelayOutcome::Persisted { .. }));
        // Same backlog head: no second alert. Repeating the same fact at the
        // daemon's scan cadence would be the noise this is meant to replace.
        let replay = enqueue_supervisor_unread_relay(&cas_dir, "daring-marten-11", 24370, 4);
        assert!(matches!(
            replay,
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        // The head moved — the supervisor read the oldest and something is
        // still waiting. That is a new fact and is reported.
        let moved = enqueue_supervisor_unread_relay(&cas_dir, "swift-fox", 24371, 2);
        assert!(matches!(moved, WorkerAttentionRelayOutcome::Persisted { .. }));

        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 2, "one relay per distinct backlog head");
        for row in &rows {
            assert_eq!(row.target, "supervisor");
            // Must satisfy the real wake gate's Daemon class, or the fail-safe
            // is as silent as the traffic it is reporting.
            assert_eq!(row.origin, Some(cas_store::QueueOrigin::Daemon));
            assert!(
                crate::mcp::tools::core::task::lifecycle::supervisor_push::is_lifecycle_wake_source(
                    &row.source
                )
            );
            assert!(crate::prompt_revalidation::is_supervisor_wake_envelope(
                &row.prompt
            ));
        }
        assert!(
            rows.iter().any(|row| row.prompt.contains("3 messages")
                && row.prompt.contains("24370")
                && row.prompt.contains("daring-marten-11")),
            "the relay must name the count, the backlog head and its sender: {rows:?}"
        );
    }

    #[test]
    fn merge_queue_ejection_relays_to_supervisor_and_worker_once_and_notes_task() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "merge-queue-ejection-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "merge-queue-ejection-test");
        let task_store = crate::store::open_task_store_cached(&cas_dir).unwrap();
        let mut task = cas_types::Task::new("cas-fc35".to_string(), "parked delivery".to_string());
        task.status = cas_types::TaskStatus::AwaitingMerge;
        task.assignee = Some("fast-jaguar-59".to_string());
        task_store.add(&task).unwrap();

        let first = enqueue_merge_queue_ejection_relay(
            &cas_dir,
            "cas-fc35",
            "fast-jaguar-59",
            556,
            Some(32386300052),
            "merge-group-run:32386300052",
            Some("Fast Validation — CI tiers › Test release publication guards"),
        );
        let replay = enqueue_merge_queue_ejection_relay(
            &cas_dir,
            "cas-fc35",
            "fast-jaguar-59",
            556,
            Some(32386300052),
            "merge-group-run:32386300052",
            Some("Fast Validation — CI tiers › Test release publication guards"),
        );
        assert!(matches!(
            first,
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        assert!(matches!(
            replay,
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(
            rows.len(),
            2,
            "one supervisor relay plus one worker inbox row"
        );
        // cas-fd0a: one typed wake naming the PR and the failing job.
        assert!(rows.iter().any(|row| row.target == "supervisor"
            && row.prompt.contains("failed merge_group run 32386300052")
            && row.prompt.contains("PR #556")
            && row.prompt.contains(
                "Failing job: Fast Validation — CI tiers › Test release publication guards."
            )));
        assert!(
            rows.iter()
                .any(|row| row.target == "fast-jaguar-59" && row.prompt.contains("PR #556"))
        );
        let persisted = task_store.get("cas-fc35").unwrap();
        assert_eq!(
            persisted
                .notes
                .matches("merge-queue-ejection occurrence=merge-group-run:32386300052")
                .count(),
            1
        );
    }

    #[test]
    fn pr_lane_failure_relay_names_pr_check_and_run_and_replays_once() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "pr-lane-failure-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "pr-lane-failure-test");
        let failure = crate::ui::factory::daemon::runtime::ci_watch::PrLaneFailure {
            task_id: "cas-pr-lane".to_string(),
            worker: "bright-otter".to_string(),
            pr_number: 659,
            head_sha: "current-head".to_string(),
            run_id: 33436155392,
            run_url: "https://github.test/runs/33436155392".to_string(),
            check_name: crate::ui::factory::daemon::runtime::ci_watch::REQUIRED_PR_LANE_CHECK
                .to_string(),
        };

        let first = enqueue_pr_lane_failure_relay(&cas_dir, &failure);
        let replay = enqueue_pr_lane_failure_relay(&cas_dir, &failure);
        assert!(matches!(
            first,
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        assert!(matches!(
            replay,
            WorkerAttentionRelayOutcome::Persisted { .. }
        ));
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 1, "replayed PR/head failure stays idempotent");
        assert!(rows[0].prompt.contains("kind=\"pr_lane_failed\""));
        assert!(rows[0].prompt.contains("PR #659"));
        assert!(rows[0].prompt.contains("cas-pr-lane"));
        assert!(rows[0].prompt.contains("Scoped Validation (factory/PR)"));
        assert!(rows[0].prompt.contains("33436155392"));
        assert_recovery_class_requires_daemon(&rows[0]);
    }

    #[test]
    fn merged_delivery_records_receipt_wakes_worker_and_escalates_once() {
        let _env = crate::test_support::TestEnvGuard::with_vars(&[(
            "CAS_FACTORY_SESSION",
            "delivery-pr-merged-test",
        )]);
        let temp = tempfile::TempDir::new().unwrap();
        let cas_dir = crate::store::init_cas_dir(temp.path()).unwrap();
        register_supervisor(&cas_dir, "delivery-pr-merged-test");
        let task_store = crate::store::open_task_store_cached(&cas_dir).unwrap();
        let mut task = cas_types::Task::new("cas-7ea6".to_string(), "delivery".to_string());
        task.status = cas_types::TaskStatus::AwaitingMerge;
        task.assignee = Some("calm-octopus-51".to_string());
        task_store.add(&task).unwrap();
        let observation = crate::ui::factory::daemon::runtime::ci_watch::DeliveryPrObservation {
            task_id: "cas-7ea6".to_string(),
            worker: "calm-octopus-51".to_string(),
            branch: "factory/calm-octopus-51".to_string(),
            pr_number: 932,
            head_sha: "worker-tip".to_string(),
            merge_commit: Some("merge-tip".to_string()),
        };

        apply_delivery_pr_observation(&cas_dir, &observation).unwrap();
        apply_delivery_pr_observation(&cas_dir, &observation).unwrap();
        let persisted = task_store.get("cas-7ea6").unwrap();
        assert_eq!(persisted.deliverables.delivery_pr_number, Some(932));
        assert_eq!(
            persisted.deliverables.delivery_pr_merge_commit.as_deref(),
            Some("merge-tip")
        );
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].target, "calm-octopus-51");
        assert!(rows[0].prompt.contains("PR #932 merged at merge-tip"));
        assert!(rows[0].prompt.contains("close cas-7ea6 with commit_receipt=merge-tip"));
        assert_eq!(rows[0].origin, Some(cas_store::QueueOrigin::Daemon));

        let mut aged = persisted;
        aged.deliverables.delivery_pr_merged_at =
            Some(chrono::Utc::now() - chrono::Duration::minutes(10));
        task_store.update(&aged).unwrap();
        relay_delivery_pr_merge_timeouts(&cas_dir);
        relay_delivery_pr_merge_timeouts(&cas_dir);
        let rows = queue.peek_all(10).unwrap();
        assert_eq!(rows.len(), 2);
        let supervisor_row = rows.iter().find(|row| row.target == "supervisor").unwrap();
        assert!(supervisor_row.prompt.contains("kind=\"delivery_pr_merge_timeout\""));
        assert!(supervisor_row.prompt.contains("PR #932"));
        assert!(crate::prompt_revalidation::is_supervisor_wake_envelope(
            &supervisor_row.prompt
        ));
    }
}

impl FactoryDaemon {
    pub fn new(config: DaemonConfig) -> anyhow::Result<Self> {
        crate::ui::factory::session::validate_daemon_start(
            &config.session_name,
            &config.factory_config.cwd,
        )?;
        // Get initial terminal size (default for daemon without terminal)
        let (cols, rows) = (120, 40);

        // Extract fields before factory_config is moved
        let project_dir = config.factory_config.cwd.to_string_lossy().to_string();
        let lead_session_id = config.factory_config.lead_session_id.clone();
        let session_summarizer = super::session_summarizer::SessionSummarizer::new(
            config.factory_config.ai_enrichment.clone(),
        );

        // Set factory session env var so PTY children (and their MCP servers) inherit it.
        // SAFETY: called before spawning any threads or async tasks in this process.
        unsafe { std::env::set_var("CAS_FACTORY_SESSION", &config.session_name) };

        // Create the factory app (this spawns Claude instances)
        let mut app = FactoryApp::new(config.factory_config)?;
        app.set_factory_session(config.session_name.clone());

        // Track factory session start
        crate::telemetry::track_factory_started("supervisor", app.worker_names().len());
        let initial_workers = app.worker_names().len().to_string();
        crate::telemetry::track(
            "factory_session_started",
            vec![
                ("mode", "daemon"),
                (
                    "worktrees_enabled",
                    if app.worktrees_enabled() {
                        "true"
                    } else {
                        "false"
                    },
                ),
                ("initial_workers", &initial_workers),
            ],
        );

        // Create socket
        let sock_path = socket_path(&config.session_name);

        // Ensure parent directory exists
        if let Some(parent) = sock_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Create listener
        let listener = bind_factory_socket(&sock_path)?;
        listener.set_nonblocking(true)?;

        // Create GUI socket (for desktop GUI clients using JSON protocol)
        let gui_sock_path = gui_socket_path(&config.session_name);
        let gui_listener = bind_factory_socket(&gui_sock_path)?;
        gui_listener.set_nonblocking(true)?;

        // Bind WebSocket listener on localhost with OS-assigned port.
        // Use std TcpListener first (sync constructor), then convert to tokio in run().
        let ws_listener = match std::net::TcpListener::bind("127.0.0.1:0") {
            Ok(l) => {
                l.set_nonblocking(true)?;
                match tokio::net::TcpListener::from_std(l) {
                    Ok(tl) => Some(tl),
                    Err(e) => {
                        tracing::warn!("Failed to convert WS listener to tokio: {}", e);
                        None
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to bind WS listener: {}", e);
                None
            }
        };
        let ws_port = ws_listener
            .as_ref()
            .and_then(|l| l.local_addr().ok())
            .map(|a| a.port());

        let session_manager = SessionManager::new();

        // Optionally start cloud phone-home client
        let cloud_handle = if config.phone_home {
            Self::try_start_cloud_client(&config.session_name)
        } else {
            None
        };

        // Remove orphaned team directories from previous crashed sessions
        super::teams::TeamsManager::cleanup_orphans();

        // Initialize native Agent Teams for inter-agent messaging (Claude CLI only).
        let teams = {
            let tm = super::teams::TeamsManager::new(&config.session_name);
            let worker_cwds: std::collections::HashMap<String, std::path::PathBuf> = app
                .worktree_manager()
                .map(|mgr| {
                    app.worker_names()
                        .iter()
                        .map(|name| (name.clone(), mgr.worktree_path_for_worker(name)))
                        .collect()
                })
                .unwrap_or_default();
            let lead_sid = lead_session_id.as_deref().unwrap_or(&config.session_name);
            match tm.init_team_config(
                app.worker_names(),
                app.project_path(),
                &worker_cwds,
                lead_sid,
            ) {
                Ok(()) => Some(tm),
                Err(e) => {
                    tracing::error!("Failed to init Teams config: {}", e);
                    None
                }
            }
        };

        // Bind notification socket for instant prompt queue wakeup
        let notify_rx = match cas_factory::DaemonNotifier::bind(app.cas_dir()) {
            Ok(n) => Some(n),
            Err(e) => {
                tracing::warn!(
                    "Failed to create notification socket, falling back to polling: {}",
                    e
                );
                None
            }
        };

        // Save session metadata (after teams init so team_name is included)
        let mut metadata = create_metadata(
            &config.session_name,
            std::process::id(),
            app.supervisor_name(),
            app.worker_names(),
            app.epic_state().epic_id(),
            Some(&project_dir),
            ws_port,
        );
        metadata.team_name = teams.as_ref().map(|t| t.team_name().to_string());
        session_manager.save_daemon_metadata(metadata, app.cas_dir())?;

        // cas-d2a2: the prompt queue's store work runs on its own job lanes.
        let queue_jobs = crate::ui::factory::daemon::runtime::prompt_queue_jobs::QueueJobs::start(app.cas_dir().to_path_buf(), app.project_scope());
        Ok(Self {
            session_name: config.session_name,
            app,
            listener,
            clients: HashMap::new(),
            next_client_id: 0,
            owner_client_id: None,
            owner_last_activity: Instant::now(),
            session_manager,
            shutdown: Arc::new(AtomicBool::new(false)),
            cols,
            rows,
            pending_resize: None,
            pending_resize_at: Instant::now(),
            compact_terminal: None,
            compact_cols: 0,
            compact_rows: 0,
            pending_spawns: VecDeque::new(),
            binding_intake: Default::default(),
            spawn_task: None,
            spawn_verifications: HashMap::new(),
            cloud_handle,
            phone_home: false,
            relay_clients: HashMap::new(),
            pane_watchers: HashMap::new(),
            pane_buffers: HashMap::new(),
            session_summarizer,
            gui_listener,
            gui_clients: HashMap::new(),
            next_gui_client_id: 0,
            ws_listener,
            ws_clients: HashMap::new(),
            terminal_exchange: Default::default(),
            next_ws_client_id: 0,
            relay_token: None,
            web_pane_sizes: HashMap::new(),
            teams,
            notify_rx,
            dead_workers: std::collections::HashSet::new(),
            recent_worker_exits: Vec::new(),
            reported_unavailable_workers: std::collections::HashMap::new(),
            last_usage_limit_scan: None,
            open_operator_notices: std::collections::HashMap::new(),
            last_permission_request_scan: None,
            last_death_sweep: None,
            reported_permission_requests: std::collections::HashSet::new(),
            relay_jobs: Default::default(),
            last_commander_mirror_scan: None,
            work_events: Default::default(),
            queue_jobs,
        reported_auth_failed_workers: std::collections::HashMap::new(),
            last_auth_failure_scan: None,
            cancelled_spawns: std::collections::HashSet::new(),
            last_idle_message_times: HashMap::new(),
            stall_nudge_gate_checks: HashMap::new(),
            lifecycle_redelivery_attempts: HashMap::new(),
            lifecycle_redelivery_counts: HashMap::new(),
            inbox_deferred_writes: std::collections::HashMap::new(),
            urgent_wake_probes: HashMap::new(),
            urgent_injects: Default::default(),
            operator_refresh_requested: false,
            send_receipts: crate::ui::factory::daemon::runtime::send_dedupe::SendReceipts::default(),
            supervisor_commands: Default::default(),
            normal_delivery_probes: HashMap::new(),
            last_pane_output_bytes: HashMap::new(),
            pane_silent_since: HashMap::new(),
            gate_activity_recorded_at: HashMap::new(),
            last_prompt_poison_sweep: Some(Instant::now()),
            resumed_epic_ids: std::collections::HashSet::new(),
            spawn_started_at: None,
            spawn_cancellation: None,
            last_spawn_queue_stall_scan: None,
            external_wake: Default::default(),
            reminder_scan_generation: 0,
            pending_ci_wakes: Vec::new(),
            reported_stalled_spawn_requests: std::collections::HashSet::new(),
        })
    }

    /// Get the shutdown flag for external control
    pub fn shutdown_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.shutdown)
    }

    /// Run the daemon main loop with TUI rendering
    pub async fn run(&mut self) -> anyhow::Result<()> {
        #[cfg(unix)]
        let _signals = super::super::process::shutdown_on_signals(Arc::clone(&self.shutdown))?;
        crate::factory_build_memory::protect_interactive_memory();
        // Backstop for every entry path: a gone client must not kill the
        // daemon (cas-5918). The process entry points set it earlier.
        crate::server_signals::ignore_sigpipe_for_server();
        // GH #1363: daemon.log otherwise holds only stderr, so a healthy
        // session that spawned no worker left it empty.
        crate::ui::factory::record_daemon_lifecycle(
            &self.session_name,
            "started",
            &format!(
                "daemon loop starting; runtime tracing goes to {}",
                self.app.cas_dir().join("logs").display()
            ),
        );
        // cas-ca22: mint this run's Commander relay token into the hub's
        // private directory; only the hub relay presenting it may carry
        // operator_verified. Without it every client is stored unverified.
        match crate::hub::HubRuntimePaths::default_for_user().and_then(|paths| {
            crate::ui::factory::relay_token::mint(paths.root(), &self.session_name)
        }) {
            Ok(token) => self.relay_token = Some(token),
            Err(error) => tracing::warn!(
                %error,
                "cas-ca22: no Commander relay token; operator messages will be stored unverified"
            ),
        }
        // Bind WebSocket listener if not already bound (fork-first and legacy paths
        // set ws_listener=None because they run before the Tokio runtime exists).
        if self.ws_listener.is_none() {
            match std::net::TcpListener::bind("127.0.0.1:0") {
                Ok(l) => {
                    let _ = l.set_nonblocking(true);
                    match tokio::net::TcpListener::from_std(l) {
                        Ok(tl) => {
                            let port = tl.local_addr().ok().map(|a| a.port());
                            self.ws_listener = Some(tl);
                            // Update session metadata with ws_port
                            if let Some(port) = port {
                                let meta_path =
                                    crate::ui::factory::session::metadata_path(&self.session_name);
                                if let Ok(data) = std::fs::read_to_string(&meta_path) {
                                    if let Ok(mut meta) =
                                        serde_json::from_str::<
                                            crate::ui::factory::protocol::SessionMetadata,
                                        >(&data)
                                    {
                                        meta.ws_port = Some(port);
                                        let _ = self.session_manager.save_metadata(&meta);
                                    }
                                }
                                tracing::info!("WS listener bound on 127.0.0.1:{}", port);
                            }
                        }
                        Err(e) => {
                            tracing::warn!("Failed to convert WS listener to tokio: {}", e);
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to bind WS listener: {}", e);
                }
            }
        }

        // Start deferred cloud phone-home client (fork-first path defers this
        // because run_with_progress() runs before the Tokio runtime exists).
        if self.phone_home && self.cloud_handle.is_none() {
            self.cloud_handle = Self::try_start_cloud_client(&self.session_name);
        }

        let session_started_at = Instant::now();

        // Create buffer backend for rendering
        let backend = BufferBackend::with_hyperlinks(
            self.cols,
            self.rows,
            self.app.full_pane_hyperlink_map(),
        );
        let mut terminal = Terminal::new(backend)?;

        // Set initial terminal title
        set_terminal_title(self.app.project_path(), self.app.epic_state().epic_title());

        // Start recording if enabled
        if self.app.record_enabled() {
            if let Err(e) = self.app.start_recording().await {
                tracing::error!("Failed to start recording: {}", e);
            }
        }

        // Refresh intervals
        let mut last_refresh = std::time::Instant::now();
        let mut last_prompt_poll = std::time::Instant::now();
        // GitHub polling is intentionally decoupled from the two-second UI
        // refresh. The blocking CLI calls run on Tokio's blocking pool and
        // results are handed back to this loop for durable prompt enqueue.
        let mut last_ci_watch = std::time::Instant::now()
            .checked_sub(super::ci_watch::CI_WATCH_INTERVAL)
            .unwrap_or_else(std::time::Instant::now);
        let mut ci_watch_task: Option<
            JoinHandle<
                Result<
                    (
                        Vec<super::ci_watch::CiFailure>,
                        super::ci_watch::MergeQueuePoll,
                        Vec<(
                            super::ci_watch::MergeQueueEjection,
                            Option<String>,
                        )>,
                        Vec<super::ci_watch::DeliveryPrObservation>,
                        (Vec<super::ci_watch::CiWake>, bool),
                    ),
                    super::ci_watch::CiWatchError,
                >,
            >,
        > = None;
        let mut ci_watch_unavailable_reported = false;
        // cas-0f05: poll faster while a `ci_completed` reminder is pending.
        let mut ci_wake_pending = false;
        // Queue membership is intentionally process-local: the durable relay
        // key is the recovery boundary. A failed merge_group run also lets the
        // first poll after a daemon restart recover a real ejection.
        let mut last_merge_queue_membership = std::collections::BTreeSet::new();
        // Track the arm independently from queue membership. A clean PR can
        // lose both its auto-merge arm and queue entry without a merge-group
        // run, which still needs one supervisor wake.
        let mut last_auto_merge_membership = std::collections::BTreeSet::new();
        // cas-e753 (GH #1145): Violet push-wake. Claims run on the blocking
        // pool every 15 s; channel sweeps run as their own task so a slow
        // Violet read never holds up the next claim.
        let violet_wake = super::violet_activity::VioletWakeRuntime::start(
            self.app.cas_dir(),
            &self.session_name,
            self.app.supervisor_name(),
        )
        .map(std::sync::Arc::new);
        let mut last_violet_poll = std::time::Instant::now()
            .checked_sub(super::violet_activity::POLL_INTERVAL)
            .unwrap_or_else(std::time::Instant::now);
        let mut violet_error_reported = false;
        // cas-f9c7: messages from peer supervisors of this repo on other
        // machines, claimed from the Cassy Cloud peer mailbox every 15 s on
        // the blocking pool.
        let mut peer_mailbox = crate::cloud::peer_mailbox::PeerMailboxPoller::new(
            self.app.cas_dir(),
            &self.session_name,
        );
        let refresh_interval = DAEMON_REFRESH_INTERVAL;
        let poll_interval = Duration::from_millis(100);

        let mut prompt_notified = false;

        // cas-73b5 (GH #970): the loop records its progress; an OS thread
        // publishes it for `worker_status`, so a blocked pass is still seen.
        let loop_progress = super::loop_watchdog::LoopProgress::new();
        // cas-d2a2: the prompt queue's job lanes go into `loop.json` and the
        // wedge check.
        if let Some(lanes) = self.queue_jobs.job_lanes() {
            loop_progress.watch_jobs(lanes);
        }
        let _loop_watchdog = super::loop_watchdog::spawn_watchdog(
            std::sync::Arc::clone(&loop_progress),
            self.app.cas_dir().to_path_buf(),
            self.session_name.clone(),
            std::sync::Arc::clone(&self.shutdown),
        );
        // cas-77f39 (GH #1210, factory loop v2 §5): output frames are capped
        // and coalesced; input echo is drawn at once. The caps are read once,
        // before this thread becomes a lane.
        // What the TUI draws besides pane contents, as of the last pass.
        let mut last_render_fingerprint = self.app.render_fingerprint();
        let mut render_scheduler = super::render_scheduler::RenderScheduler::new(
            super::render_scheduler::RenderCaps::from_config(
                &crate::config::Config::load(self.app.cas_dir()).unwrap_or_default(),
            ),
        );
        // cas-6931 (factory loop v2 §6): this thread is the daemon's io and
        // control lane. Steps marked below and inside the prompt queue and
        // the director apply reach the watchdog, and blocking calls made on
        // this thread are reported (report mode).
        // cas-786a6: the loop's prompt-queue handle is opened (schema init
        // once) before this thread becomes a lane, and claims a previous
        // daemon never handed off are released.
        self.release_orphaned_claims_at_start();
        let _loop_lane = loop_progress.attach(super::loop_watchdog::Lane::Control);
        let mut last_reset_check = std::time::Instant::now();
        // GH #1165: store writes whose result the loop does not need on the
        // spot run on one ordered background thread, and the task-sync repair
        // the cached task store skips on open runs on its own schedule.
        let store_worker = super::store_worker::global();
        // cas-ee9ab / cas-223f: the refresh's store reads, health probes and
        // re-checks run as jobs on the daemon's one job runtime (the queue's),
        // never on this loop. Stall confirmation asks for probes instead of
        // running them.
        let mut director_refresh = super::director_refresh::DirectorRefresh::new();
        self.app.set_deferred_worker_health_probes(true);
        // cas-5e72: session-metadata writes (epic focus, worker holds) run on
        // the store worker.
        self.app.set_deferred_session_metadata_writes(true);
        let _task_sync_reconcile = super::store_worker::spawn_task_sync_reconcile(
            self.app.cas_dir().to_path_buf(),
            Arc::clone(&self.shutdown),
        );

        while !self.shutdown.load(Ordering::Relaxed) {
            // GH #1165: every store wait left on this thread (SQLite busy
            // handler and retries, the connection mutex, the task-sync flock)
            // shares one per-pass budget and fails fast past it. The loop
            // retries on a later pass and panels keep their last snapshot.
            // `run` is driven by `Runtime::block_on`, so this thread-local
            // scope stays on this thread across the awaits below.
            loop_progress.begin_pass();
            let pass_store_budget = cas_store::wait_budget::bound_waits_for(
                super::store_worker::PASS_STORE_WAIT_BUDGET,
            );
            loop_progress.enter(super::loop_watchdog::LoopPhase::ClientInput);
            // Input forwarding and PTY exchange must not touch the store.
            let store_free_input = cas_store::wait_budget::forbid_store_access();
            // C6 (cas-9db0b): input forwarding and the PTY exchange are proven
            // clean, so this lane enforces here. Outside tests a blocking call
            // is recorded and logged once as an error; it never panics.
            let enforced_input = cas_store::lane_guard::enforce_here();
            // Error timeout must run in daemon mode too (not only local event loop path).
            let had_error = self.app.error_message.is_some();
            self.app.check_error_timeout();
            let error_cleared_by_timeout = had_error && self.app.error_message.is_none();

            // Accept new client connections (non-blocking)
            let new_clients = self.accept_clients()?;

            // Accept new GUI client connections (non-blocking)
            let new_gui_clients = self.accept_gui_clients();

            // Accept new WebSocket client connections (non-blocking)
            let new_ws_clients = self.accept_ws_clients().await;

            // Read and process input from clients
            let input_activity = self.process_client_input().await?;
            // cas-77f39: output from the pane that took input is its echo.
            let input_pane = input_activity
                .then(|| self.app.mux.focused_id().map(str::to_string))
                .flatten();

            // Read and process input from GUI clients
            let gui_activity = self.process_gui_client_input().await;

            // Keyframe requests join the terminal exchange, which owns capture,
            // PTY drain and frame enqueue under this loop's exclusive borrow.
            let ws_activity = self.process_ws_client_input().await;
            let bytes_processed = self.exchange_terminal(&loop_progress).await;
            let had_output = bytes_processed > 0;
            drop(enforced_input);
            drop(store_free_input);

            let summary_metadata = format!(
                "session={} role=supervisor task={}",
                self.session_name,
                self.app.epic_state().epic_title().unwrap_or("unassigned")
            );
            if let Some(summary) = self
                .session_summarizer
                .poll(&self.pane_buffers, &summary_metadata)
                .await
            {
                self.broadcast_daemon_message(
                    &crate::ui::factory::protocol::DaemonMessage::SessionSummary { summary },
                );
            }

            // Process relay events from cloud (remote terminal attach/input/detach)
            loop_progress.enter(super::loop_watchdog::LoopPhase::Relay);
            self.process_relay_events().await;

            // cas-1a4d: retry non-urgent PTY injects only after every attached
            // client surface had a chance to submit or clear its composer.
            // This stays outside process_prompt_queue so a deferred inject
            // never blocks the input loop that can make its target clean.
            // C6 (cas-9db0b): the C4a inject and C3 queue pass are proven
            // clean; the lane enforces until the queue pass ends.
            let enforced_queue = cas_store::lane_guard::enforce_here();
            self.app.mux.flush_deferred_injections().await;
            // cas-786a6 (factory loop v2 C4a): urgent injects whose pane has
            // settled type their payload now. A settle is a per-pane state;
            // no pass waits for one.
            self.advance_urgent_injects().await;

            // Poll prompt queue (on notification or timer)
            loop_progress.enter(super::loop_watchdog::LoopPhase::PromptQueue);
            let prompt_tick = prompt_notified || last_prompt_poll.elapsed() >= poll_interval;
            if !prompt_tick && self.queue_jobs.has_ready() {
                // cas-d2a2: a QueueSelect finished between ticks; deliver it
                // now rather than on the next tick.
                let _ = self.process_prompt_queue().await;
            }
            if prompt_tick {
                if prompt_notified {
                    if let Some(ref mut notify) = self.notify_rx {
                        notify.drain();
                    }
                }
                let _ = self.process_prompt_queue().await;
                // The pass's sub-steps end here (cas-6931).
                loop_progress.enter(super::loop_watchdog::LoopPhase::PromptQueue);
                // cas-ecff: auto-drain pending lifecycle outbox (durable
                // task_lifecycle rows with prompt_delivered_at unset) so
                // partial failures recover without re-running task mutations.
                // GH #1165: on the store worker, at most one drain at a time.
                let cas_dir = self.app.cas_dir().to_path_buf();
                store_worker.submit_unique("lifecycle outbox drain", move || {
                    drain_lifecycle_outbox_once(&cas_dir)
                });
                last_prompt_poll = std::time::Instant::now();
                prompt_notified = false;
            }
            drop(enforced_queue);

            // cas-73b5: a supervisor's `restart_spawn_queue` request is applied
            // here, at most once a second.
            loop_progress.enter(super::loop_watchdog::LoopPhase::SpawnQueue);
            if last_reset_check.elapsed() >= Duration::from_secs(1) {
                last_reset_check = std::time::Instant::now();
                if let Some(request) =
                    crate::factory_daemon_health::take_reset(self.app.cas_dir(), &self.session_name)
                {
                    let outcome = self.apply_spawn_queue_reset(&request);
                    loop_progress.record_reset(outcome);
                }
            }

            // Poll spawn queue (enqueues requests, doesn't execute them)
            // Completion-driven acceptance; the ingress owns the 100ms cadence.
            if let Err(error) = self.enqueue_spawn_requests() {
                tracing::error!(%error, "cas-2702: could not accept spawn queue poll result");
            }

            // Process pending spawns (non-blocking: git ops run on background thread)
            loop_progress.enter(super::loop_watchdog::LoopPhase::PendingSpawns);
            if self.spawn_task.is_some() || !self.pending_spawns.is_empty() {
                self.process_pending_spawns().await;
            }

            // Periodic Cassy data refresh
            loop_progress.enter(super::loop_watchdog::LoopPhase::Refresh);
            let mut refreshed = false;
            // cas-786a6: Ctrl+R asks for the next snapshot now.
            let refresh_due = last_refresh.elapsed() >= refresh_interval
                || std::mem::take(&mut self.operator_refresh_requested);
            if refresh_due || director_refresh.has_finished(&mut self.queue_jobs) {
                // Collect a completed GitHub Actions snapshot in the background.
                // No supervisor action is required to notice a red run: the
                // completed result below becomes a lifecycle-wake relay.
                if ci_watch_task.is_none()
                    && last_ci_watch.elapsed()
                        >= super::ci_watch::ci_watch_interval(
                            ci_wake_pending,
                            !last_merge_queue_membership.is_empty(),
                        )
                {
                    let session_name = self.session_name.clone();
                    let project = self.app.project_path().to_path_buf();
                    // GH #1165: the AwaitingMerge read runs with the GitHub
                    // calls on the blocking pool, not on this loop.
                    let watch_cas_dir = self.app.cas_dir().to_path_buf();
                    let previously_queued = last_merge_queue_membership.clone();
                    let previously_armed = last_auto_merge_membership.clone();
                    let mut watched_branches =
                        std::collections::BTreeSet::from(["main".to_string()]);
                    if let Some(manager) = self.app.worktree_manager() {
                        watched_branches.extend(
                            self.app
                                .worker_names()
                                .iter()
                                .map(|worker| manager.branch_name_for_worker(worker)),
                        );
                    } else if let Ok(output) = Command::new("git")
                        .args(["rev-parse", "--abbrev-ref", "HEAD"])
                        .current_dir(&project)
                        .output()
                        && output.status.success()
                    {
                        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
                        if branch.starts_with("factory/") {
                            watched_branches.insert(branch);
                        }
                    }
                    ci_watch_task = Some(tokio::task::spawn_blocking(move || {
                        let deliveries = awaiting_merge_deliveries(&watch_cas_dir);
                        let supervisor_prs =
                            super::merge_queue_ejection::supervisor_prs(&watch_cas_dir);
                        let transport = super::ci_watch::GhCiTransport::from_project(&project)?;
                        let candidates = super::queue_and_events::ci_completed_candidates(
                            &watch_cas_dir,
                            &session_name,
                        );
                        let ci_wakes = (
                            super::ci_watch::collect_ci_wakes(&transport, &candidates),
                            !candidates.is_empty(),
                        );
                        let failures = super::ci_watch::mark_merged_failures(
                            &project,
                            super::ci_watch::collect_failures(&transport, &watched_branches)?,
                        );
                        let mut queue_poll = super::ci_watch::collect_tracked_merge_queue_ejections(
                            &transport,
                            &deliveries,
                            &supervisor_prs,
                            &previously_queued,
                            &previously_armed,
                        )?;
                        // Reuse this existing bounded background read. Presence
                        // publishes the local snapshot; the control loop does
                        // no inventory I/O and adds no GitHub request.
                        crate::cloud::peers_detail::save_pr_snapshot(
                            &watch_cas_dir, std::mem::take(&mut queue_poll.observed_prs), chrono::Utc::now(),
                        );
                        // cas-fd0a: name each ejection's failing job for the
                        // wake. GitHub calls stay on this blocking task
                        // (GH #1165).
                        let ejections = std::mem::take(&mut queue_poll.ejections)
                            .into_iter()
                            .map(|ejection| {
                                let job = super::merge_queue_ejection::failing_job(
                                    &transport,
                                    ejection.failed_run_id,
                                );
                                (ejection, job)
                            })
                            .collect::<Vec<_>>();
                        let delivery_pr_observations =
                            super::ci_watch::collect_delivery_pr_observations(
                                &transport,
                                &deliveries,
                            )?;
                        Ok((failures, queue_poll, ejections, delivery_pr_observations, ci_wakes))
                    }));
                    last_ci_watch = std::time::Instant::now();
                }

                if ci_watch_task.as_ref().is_some_and(JoinHandle::is_finished) {
                    let task = ci_watch_task.take().expect("checked above");
                    match task.await {
                        Ok(Ok((
                            failures,
                            queue_poll,
                            ejections,
                            delivery_pr_observations,
                            (ci_wakes, pending),
                        ))) => {
                            ci_watch_unavailable_reported = false;
                            ci_wake_pending = pending;
                            self.pending_ci_wakes.extend(ci_wakes);
                            last_merge_queue_membership = queue_poll.queued_prs;
                            last_auto_merge_membership = queue_poll.auto_merge_prs;
                            // GH #1165: persisting the results is store work;
                            // it runs on the store worker, in order.
                            let cas_dir = self.app.cas_dir().to_path_buf();
                            let session_name = self.session_name.clone();
                            let lane_failures = queue_poll.pr_lane_failures;
                            store_worker.submit("ci watch results", move || {
                                persist_ci_watch_results(
                                    &cas_dir,
                                    &session_name,
                                    delivery_pr_observations,
                                    ejections,
                                    lane_failures,
                                    failures,
                                )
                            });
                        }
                        Ok(Err(error)) => {
                            if !ci_watch_unavailable_reported {
                                tracing::warn!(%error, "GitHub CI watcher unavailable; retrying silently on its next cadence");
                                ci_watch_unavailable_reported = true;
                            }
                        }
                        Err(error) => {
                            tracing::warn!(%error, "GitHub CI watcher background task failed")
                        }
                    }
                }

                self.binding_intake.reap_sweep().await;
                if let Some(violet) = violet_wake.as_ref() {
                    // cas-9021: each tick and sweep reads the durable intake
                    // gate per channel; the v2 managed reader is never gated.
                    if self.binding_intake.tick.is_none()
                        && last_violet_poll.elapsed() >= super::violet_activity::POLL_INTERVAL
                    {
                        let violet = std::sync::Arc::clone(violet);
                        self.binding_intake.tick =
                            Some(tokio::task::spawn_blocking(move || violet.tick_blocking()));
                        last_violet_poll = std::time::Instant::now();
                    }
                    if self.binding_intake.tick.as_ref().is_some_and(JoinHandle::is_finished) {
                        match self.binding_intake.tick.take().expect("checked above").await {
                            Ok((report, sweeps, presence)) => {
                                if report.wakes > 0 {
                                    super::delivery::wake_daemon_after_enqueue(self.app.cas_dir());
                                    tracing::info!(
                                        wakes = report.wakes,
                                        admitted = report.admitted,
                                        "queued Violet Slack-activity wake for supervisor"
                                    );
                                }
                                if !report.denied.is_empty() {
                                    tracing::debug!(denied = ?report.denied, "Cloud denied Violet activity claim for project");
                                }
                                if report.errors.is_empty() {
                                    violet_error_reported = false;
                                } else if !violet_error_reported {
                                    tracing::warn!(errors = ?report.errors, "Violet activity relay unavailable; retrying silently on its next cadence");
                                    violet_error_reported = true;
                                }
                                if let Some(presence) = presence
                                    && !sweeps.is_empty()
                                    && self.binding_intake.sweep.is_none()
                                {
                                    let violet = std::sync::Arc::clone(violet);
                                    let cas_dir = self.app.cas_dir().to_path_buf();
                                    self.binding_intake.sweep = Some(tokio::spawn(async move {
                                        if violet.sweep(sweeps, presence).await? > 0 {
                                            super::delivery::wake_daemon_after_enqueue(&cas_dir);
                                        }
                                        Ok(())
                                    }));
                                }
                            }
                            Err(error) => {
                                self.binding_intake.failed(format!("legacy tick did not finish: {error}"));
                                tracing::warn!(%error, "Violet activity poll task failed")
                            }
                        }
                    }
                }

                self.finish_binding_intake_replies();

                if let Some(report) = peer_mailbox.poll(std::time::Instant::now()).await
                    && report.delivered > 0
                {
                    super::delivery::wake_daemon_after_enqueue(self.app.cas_dir());
                    tracing::info!(
                        delivered = report.delivered,
                        rejected = report.rejected,
                        "queued peer supervisor messages from Cassy Cloud"
                    );
                }

                // A successful PTY spawn is not a verified worker. Confirm the
                // harness reached Cassy registration (or surface a bounded
                // timeout) on the existing two-second lifecycle cadence.
                loop_progress.enter(super::loop_watchdog::LoopPhase::RefreshSpawnVerify);
                self.reconcile_spawn_verifications().await;
                // cas-f9e8 telemetry: the gap between the previous refresh
                // and this one is Channel C's worst-case delivery latency
                // for director-generated events. Logged at debug; enable
                // via `RUST_LOG=cas::coordination=debug`.
                let tick_interval_ms = last_refresh.elapsed().as_secs_f64() * 1000.0;
                if refresh_due {
                    last_refresh = std::time::Instant::now();
                    // cas-1b4d: a closed rapid-response window back-fills its
                    // skipped checks; the store work runs off this loop.
                    let cas_dir = self.app.cas_dir().to_path_buf();
                    if cas_dir.join(crate::rapid_response::STATE_FILE).exists() {
                        let supervisor = self.app.supervisor_name().to_string();
                        let session = self.session_name.clone();
                        tokio::task::spawn_blocking(move || {
                            super::queue_and_events::backfill_closed_rapid_response_windows(
                                &cas_dir, &supervisor, &session,
                            )
                        });
                    }
                }
                // cas-ee9ab: the director reads run on a background thread.
                // This pass starts one when due and applies whichever read
                // has finished; panels are stale by at most one refresh.
                refreshed = self
                    .director_refresh_pass_with(
                        &mut director_refresh,
                        refresh_due,
                        store_worker,
                        &loop_progress,
                        tick_interval_ms,
                    )
                    .await;
            }

            // Apply debounced resize after 100ms of no new resize events
            loop_progress.enter(super::loop_watchdog::LoopPhase::Render);
            let mut resize_applied = false;
            if let Some((cols, rows)) = self.pending_resize {
                if self.pending_resize_at.elapsed() >= Duration::from_millis(100) {
                    tracing::info!("Applying debounced resize: {}x{}", cols, rows);

                    // Determine if we have full-mode clients that need the full layout resize
                    let has_full_clients = self
                        .clients
                        .values()
                        .any(|c| c.view_mode == ClientViewMode::Full);

                    if has_full_clients {
                        // Use the largest full-mode client dimensions for the full layout
                        let (full_cols, full_rows) = self.dims_for_mode(ClientViewMode::Full);
                        if full_cols > 0 && full_rows > 0 {
                            self.cols = full_cols;
                            self.rows = full_rows;
                            let _ = self.app.handle_resize(full_cols, full_rows);
                        }
                    } else if cols >= COMPACT_WIDTH_THRESHOLD {
                        // No explicit full clients but this resize is full-sized
                        self.cols = cols;
                        self.rows = rows;
                        let _ = self.app.handle_resize(cols, rows);
                    }

                    // Update compact terminal dimensions if compact clients exist
                    let has_compact_clients = self
                        .clients
                        .values()
                        .any(|c| c.view_mode == ClientViewMode::Compact);

                    if has_compact_clients {
                        let (cc, cr) = self.dims_for_mode(ClientViewMode::Compact);
                        if cc > 0 && cr > 0 && (cc != self.compact_cols || cr != self.compact_rows)
                        {
                            self.compact_cols = cc;
                            self.compact_rows = cr;
                            // Resize supervisor PTY to fit compact layout if no full clients
                            // are connected (phone is the only viewer)
                            if !has_full_clients {
                                let sup_rows = cr.saturating_sub(1); // 1 for status bar
                                let sup_cols = cc;
                                let sup_name = self.app.supervisor_name().to_string();
                                if let Some(pane) = self.app.mux.get_mut(&sup_name) {
                                    let _ = pane.resize(sup_rows, sup_cols);
                                }
                            }
                            // Rebuild compact terminal
                            let backend = BufferBackend::with_hyperlinks(
                                cc,
                                cr,
                                self.app.compact_pane_hyperlink_map(),
                            );
                            self.compact_terminal = Some(Terminal::new(backend)?);
                        }
                    }

                    // Reconcile panes that also have GUI/WS/web viewers
                    // against the dashboard allocation.
                    self.reconcile_viewer_constrained_panes();

                    // Rebuild pane ring buffers from the virtual terminal's
                    // current state — after resize the vt reflows content to
                    // the new dimensions, so we snapshot that and re-encode
                    // as ANSI bytes. This preserves history for web viewers.
                    self.rebuild_pane_buffers_from_snapshots();

                    self.pending_resize = None;
                    resize_applied = true;
                }
            }

            // Check if full-mode clients need a full redraw
            let needs_full_redraw = self
                .clients
                .values()
                .any(|c| c.view_mode == ClientViewMode::Full && c.needs_full_redraw);
            if needs_full_redraw || resize_applied {
                // Resize the backend to match current dimensions
                terminal.backend_mut().resize(self.cols, self.rows);
                terminal.autoresize()?;
                // When a client needs a full redraw (new connection, buffer overflow),
                // reset the terminal's diff state so the next draw() produces a
                // complete frame. Without this, autoresize() is a no-op when dims
                // haven't changed, and draw() emits only a diff against the previous
                // frame—which the new client doesn't have (its screen is blank).
                if needs_full_redraw {
                    terminal.clear()?;
                }
                for client in self.clients.values_mut() {
                    if client.view_mode == ClientViewMode::Full {
                        client.needs_full_redraw = false;
                    }
                }
            }

            // Check if compact clients need a full redraw
            let needs_compact_redraw = self
                .clients
                .values()
                .any(|c| c.view_mode == ClientViewMode::Compact && c.needs_full_redraw);
            if needs_compact_redraw || resize_applied {
                if let Some(ref mut ct) = self.compact_terminal {
                    ct.backend_mut()
                        .resize(self.compact_cols, self.compact_rows);
                    ct.autoresize()?;
                    if needs_compact_redraw {
                        ct.clear()?;
                    }
                }
                for client in self.clients.values_mut() {
                    if client.view_mode == ClientViewMode::Compact {
                        client.needs_full_redraw = false;
                    }
                }
            }

            // Suppress rendering while a resize is pending (debouncing).
            // Rendering at the old size while the terminal is already a new size
            // produces visual garbage. Wait for the debounce to settle.
            let resize_pending = self.pending_resize.is_some();

            // Send periodic state updates to GUI and WS clients on refresh
            if refreshed && (!self.gui_clients.is_empty() || !self.ws_clients.is_empty()) {
                self.send_state_update();
            }

            let spawning = self.app.spawning_count > 0;
            // cas-6931: the watchdog's badge (worst slow step, or a live job
            // wedge). It changes at most once a second. This loop cannot show
            // its own wedge: when it gets here, the wedge is over.
            let loop_health_changed = self.app.set_loop_health(loop_progress.status_line());
            // cas-77f39 (GH #1210): a frame is drawn when the render scheduler
            // says so. Input and its echo draw now; other changes coalesce
            // within 16 ms; pane output is capped per focus and never draws
            // for a pane that is not on screen.
            let frame_now = std::time::Instant::now();
            if input_activity {
                render_scheduler.note_input(input_pane.as_deref(), frame_now);
            }
            for pane in self.terminal_exchange.take_output_panes() {
                let view = if !self.app.pane_on_screen(&pane) {
                    super::render_scheduler::PaneView::Hidden
                } else if self.app.mux.focused_id() == Some(pane.as_str()) {
                    super::render_scheduler::PaneView::Focused
                } else {
                    super::render_scheduler::PaneView::Unfocused
                };
                render_scheduler.note_output(&pane, view, frame_now);
            }
            let layout_changed = loop_health_changed
                || gui_activity
                || ws_activity
                || refreshed
                || new_clients
                || new_gui_clients
                || new_ws_clients
                || needs_full_redraw
                || needs_compact_redraw
                || resize_applied
                || error_cleared_by_timeout;
            // Spawn completion, crash banners and pane-set changes used to
            // ride on pane output (cas-77f39 review).
            let render_fingerprint = self.app.render_fingerprint();
            let state_changed = render_fingerprint != last_render_fingerprint;
            last_render_fingerprint = render_fingerprint;
            if layout_changed || state_changed {
                render_scheduler.note_layout();
            }
            // The spawn spinner and a loading dialog animate at <= 10 fps.
            if spawning || self.app.task_dialog_load.is_loading() {
                render_scheduler.note_animation();
            }
            // A pending resize keeps its causes for the frame after it.
            let frame = if resize_pending {
                None
            } else {
                render_scheduler.take_frame(frame_now)
            };
            let mut frame_bytes = 0usize;
            // GH #1165: drawing must not touch the store.
            let store_free_draw = cas_store::wait_budget::forbid_store_access();
            if frame.is_some() {
                // Render full TUI for full-mode clients (and relay clients)
                let has_full_clients = self
                    .clients
                    .values()
                    .any(|c| c.view_mode == ClientViewMode::Full);
                let has_relay = self.has_relay_clients();
                if has_full_clients || has_relay {
                    terminal.draw(|f| self.app.render(f))?;
                    if let Some(state) = self.app.full_host_cursor_state() {
                        terminal.backend_mut().set_cursor_style(state)?;
                    }
                    let output = terminal.backend_mut().take_buffer();
                    frame_bytes += output.len();
                    if !output.is_empty() {
                        if has_full_clients {
                            self.broadcast_output_to(&output, ClientViewMode::Full);
                        }
                        if has_relay {
                            self.broadcast_relay_output(&output);
                        }
                    }
                }

                // Render compact TUI for compact-mode clients
                let has_compact_clients = self
                    .clients
                    .values()
                    .any(|c| c.view_mode == ClientViewMode::Compact);
                if has_compact_clients {
                    if let Some(ref mut ct) = self.compact_terminal {
                        ct.draw(|f| self.app.render_compact(f))?;
                        if let Some(state) = self.app.compact_host_cursor_state() {
                            ct.backend_mut().set_cursor_style(state)?;
                        }
                        let output = ct.backend_mut().take_buffer();
                        frame_bytes += output.len();
                        if !output.is_empty() {
                            self.broadcast_output_to(&output, ClientViewMode::Compact);
                        }
                    }
                }
            }
            // An empty diff sends nothing and is not a frame.
            if let Some(frame) = frame
                && frame_bytes > 0
            {
                loop_progress.record_frame(frame.cause.name(), frame.coalesced, frame_bytes);
            }
            // Flush any pending client output even if nothing new was rendered
            if !self.clients.is_empty() && self.clients.values().any(|c| !c.output_buf.is_empty()) {
                self.flush_client_output();
            }

            // Flush pending GUI client output
            if !self.gui_clients.is_empty()
                && self.gui_clients.values().any(|c| !c.write_buf.is_empty())
            {
                self.flush_gui_client_output();
            }

            // Flush pending WebSocket client output
            if !self.ws_clients.is_empty() {
                self.deliver_work_events();
                self.flush_ws_client_output().await;
            }

            // GH #1196: an operator send can tell from loop.json that no
            // Commander client is connected.
            loop_progress.set_commander_clients(self.ws_clients.len());
            loop_progress.set_spawn_snapshot(
                self.pending_spawns.len(),
                self.spawn_task.as_ref().map(|(name, ..)| {
                    let started = self
                        .spawn_started_at
                        .map(|started| {
                            chrono::Utc::now()
                                - chrono::Duration::from_std(started.elapsed()).unwrap_or_default()
                        })
                        .unwrap_or_else(chrono::Utc::now);
                    (name.clone(), started)
                }),
            );
            drop(store_free_draw);
            loop_progress.complete_pass();
            loop_progress.enter(super::loop_watchdog::LoopPhase::Idle);
            drop(pass_store_budget);

            // Adaptive sleep: ~120fps when active, ~60fps idle with clients,
            // ~2fps headless (no clients, no GUI) to minimize CPU usage.
            let has_any_client = !self.clients.is_empty()
                || !self.gui_clients.is_empty()
                || !self.ws_clients.is_empty()
                || self.has_relay_clients();
            let sleep_ms = if had_output && has_any_client {
                4
            } else if spawning && has_any_client {
                100 // Spinner updates every 100ms
            } else if has_any_client {
                8
            } else {
                500 // Headless: no rendering needed, sleep longer
            };
            // cas-786a6: a settling urgent inject is advanced promptly; its
            // readiness is judged once per pass.
            let sleep_ms = if self.app.mux.has_pending_injects() {
                sleep_ms.min(URGENT_INJECT_POLL_MS)
            } else {
                sleep_ms
            };
            let sleep_dur = Duration::from_millis(sleep_ms);
            if let Some(ref mut notify) = self.notify_rx {
                tokio::select! {
                    result = notify.recv() => {
                        if result.is_ok() {
                            prompt_notified = true;
                        }
                    }
                    _ = tokio::time::sleep(sleep_dur) => {}
                }
            } else {
                tokio::time::sleep(sleep_dur).await;
            }
        }
        crate::ui::factory::record_daemon_lifecycle(
            &self.session_name,
            "stopping",
            "shutdown requested (supervisor exit, quit or signal; the cause is in the runtime log); tearing down",
        );

        // End only this session's Violet watches before removing its liveness
        // receipt. Mark stopping without waiting for an in-flight HTTP claim;
        // that tick finishes retirement, and later ticks cannot reopen it.
        if let Some(violet) = violet_wake.as_ref() {
            let violet = std::sync::Arc::clone(violet);
            match tokio::task::spawn_blocking(move || violet.stop_blocking()).await {
                Ok(Ok(_)) => {}
                Ok(Err(error)) => tracing::warn!(%error, "could not stop session Violet watches"),
                Err(error) => tracing::warn!(%error, "Violet session stop task failed"),
            }
        }
        // cas-ef94: a cleanly stopped daemon leaves no consumer receipt.
        super::violet_consumer::retire(self.app.cas_dir(), &self.session_name);

        // cas-223f review M4: prompts waiting for their fresh re-check are not
        // dropped at shutdown. Finished re-checks are delivered as decided;
        // ones that never reported are re-checked here, as the inject loop
        // did before cas-223f.
        self.drain_director_deliveries_for_shutdown(
            &mut director_refresh, store_worker, Duration::from_secs(10),
        ).await;
        drop(director_refresh);
        self.finish_shutdown_writes(store_worker, Duration::from_secs(5));

        // Stop recording if it was enabled
        if self.app.record_enabled() {
            if let Err(e) = self.app.stop_recording().await {
                tracing::error!("Failed to stop recording: {}", e);
            }

            // Upload recordings to cloud (best-effort, before disconnect)
            self.upload_recordings();
        }

        // Cleanup
        let cleanup_result = self.cleanup().await;

        // Shutdown is outside the control loop. Keep store I/O on Blocking;
        // retirement events preserve worker exposure after registry cleanup.
        persist_communication_session_summary(self.app.cas_dir().to_path_buf(), self.session_name.clone()).await;

        let duration_secs = session_started_at.elapsed().as_secs().to_string();
        let final_workers = self.app.worker_names().len().to_string();
        crate::telemetry::track(
            "factory_session_ended",
            vec![
                ("mode", "daemon"),
                (
                    "status",
                    if cleanup_result.is_ok() {
                        "ok"
                    } else {
                        "error"
                    },
                ),
                ("duration_secs", &duration_secs),
                ("final_workers", &final_workers),
            ],
        );

        crate::ui::factory::record_daemon_lifecycle(
            &self.session_name,
            "exited",
            &match &cleanup_result {
                Ok(()) => "teardown complete".to_string(),
                Err(error) => format!("teardown finished with an error: {error}"),
            },
        );
        cleanup_result?;

        Ok(())
    }

    /// Cleanup on shutdown
    async fn cleanup(&mut self) -> anyhow::Result<()> {
        // Stop the isolated provisioner before any asynchronous shutdown waits.
        // This group never contains the supervisor or live worker harnesses.
        self.cancel_provisioning();

        // cas-ca22: retire this run's Commander relay token.
        if self.relay_token.take().is_some()
            && let Ok(paths) = crate::hub::HubRuntimePaths::default_for_user()
        {
            crate::ui::factory::relay_token::remove(paths.root(), &self.session_name);
        }

        // Clean up notification socket
        if let Some(ref notify) = self.notify_rx {
            notify.cleanup();
        }

        // Clean up native Agent Teams directory
        if let Some(ref teams) = self.teams {
            teams.cleanup();
        }

        // Disconnect cloud phone-home client
        self.disconnect_cloud();

        // Retain the lane before worker registrations/panes are destroyed.
        if let Ok(agents) = open_agent_store(self.app.cas_dir())
            && let Ok(rows) = agents.list(None)
        {
            for agent in rows.iter().filter(|a| a.role == cas_types::AgentRole::Supervisor && a.factory_session.as_deref() == Some(self.session_name.as_str())) {
                if let Err(error) = crate::supervisor_continuity::record_end(self.app.cas_dir(), &agent.id, None) {
                    tracing::warn!(%error, "could not retain factory shutdown handoff");
                }
            }
        }

        // Kill all PTY process groups. Snapshot worker PGIDs first so their
        // durable ownership records can be removed only after death is
        // confirmed; any survivor stays visible to gc_report.
        let worker_names = self.app.worker_names().to_vec();
        let ownership = crate::ui::factory::process_groups::list(self.app.cas_dir())
            .unwrap_or_default().into_iter()
            .filter(|record| record.factory_session == self.session_name)
            .collect::<Vec<_>>();
        let mut ended = crate::ui::factory::session::SessionEndOutcome {
            at: String::new(), reason: "unknown".into(),
            workers_stopped: Vec::new(), orphans: Vec::new(), unverified: Vec::new(),
        };
        // Hooks may already have unregistered the supervisor; retain their original reason.
        if let Ok(store) = cas_store::SqliteAgentStore::open(self.app.cas_dir())
            && let Ok(rows) = store.recent_session_ends(&cas_types::Agent::get_or_generate_machine_id(), 100)
            && let Ok(bytes) = std::fs::read(crate::ui::factory::session::metadata_path(&self.session_name))
            && let Ok(metadata) = serde_json::from_slice::<crate::ui::factory::protocol::SessionMetadata>(&bytes)
            && let Some(row) = rows.into_iter().find(|row| row.factory_session.as_deref() == Some(self.session_name.as_str())
                && row.name == self.app.supervisor_name()
                && crate::ui::factory::session::supervisor_end_is_current(
                    &metadata, self.app.mux.pane_process_group_id(self.app.supervisor_name()),
                    row.pid, row.pid_starttime, &row.registered_at))
        {
            ended.reason = row.reason;
        }
        self.app.mux.kill_all();
        for record in &ownership {
            self.app.untrack_worker_process_group_if_gone(record.pgid).await;
        }
        let mut names = worker_names;
        names.extend(ownership.iter().map(|record| record.worker_name.clone()));
        names.sort(); names.dedup();
        for name in names {
            let records: Vec<_> = ownership.iter().filter(|record| record.worker_name == name).collect();
            use crate::ui::factory::process_groups::{ProcessGroupStatus, status};
            if records.iter().any(|record| status(record) == ProcessGroupStatus::Live) {
                ended.orphans.push(name);
            } else if !records.is_empty() && records.iter().all(|record| status(record) == ProcessGroupStatus::Gone) {
                ended.workers_stopped.push(name);
            } else {
                ended.unverified.push(name);
            }
        }
        ended.at = chrono::Utc::now().to_rfc3339();
        if let Err(error) = self.session_manager.record_ended(&self.session_name, ended) {
            tracing::warn!(%error, "could not retain Commander session-end receipt");
        }

        // cas-c5ab: a shutdown ends every hold, so re-arm held workers'
        // reminders before their agents are unregistered.
        self.app.rearm_held_reminders_at_shutdown(&self.session_name);

        // Unregister all factory agents (supervisor + workers)
        if let Ok(agent_store) = open_agent_store(self.app.cas_dir()) {
            // Collect all agent names to unregister
            let mut names_to_unregister = vec![self.app.supervisor_name().to_string()];
            names_to_unregister.extend(self.app.worker_names().iter().cloned());

            // Query agent store directly instead of using cached director_data
            // to ensure we find all agents, including those registered after cache refresh
            if let Ok(all_agents) = agent_store.list(None) {
                for name in &names_to_unregister {
                    for agent in all_agents.iter().filter(|a| {
                        a.name == *name
                            && a.factory_session.as_deref() == Some(self.session_name.as_str())
                    }) {
                        if let Err(e) = agent_store.unregister(&agent.id) {
                            tracing::warn!("Failed to unregister agent {}: {}", name, e);
                        }
                    }
                }
            }
        }

        // Remove session metadata
        self.session_manager.remove_metadata(&self.session_name)?;

        // Listener fields still hold both sockets until this daemon is dropped.
        // The kill caller (after exit), or subsequent discovery, reclaims them.
        // Drop WebSocket clients (connections will close on drop)
        self.ws_clients.clear();

        // Send leave alternate screen to all clients
        let cleanup = b"\x1b[?25h\x1b[?1049l";
        for client in self.clients.values_mut() {
            let _ = client.stream.write_all(cleanup);
        }

        Ok(())
    }
}

impl FactoryDaemon {
    /// cas-d2a2: retract stale alerts from the supervisor's Teams inbox as a
    /// job (factory loop v2 §4.3). The decision comes from a fresh read
    /// issued for it on the queue jobs' read connection, never the cached
    /// director snapshot (app/mod.rs:1541–1547); only the exact rows judged
    /// stale are removed, under the inbox lock.
    pub(super) fn submit_supervisor_inbox_retraction(&mut self) {
        let Some(teams) = self.teams.as_ref() else {
            return;
        };
        let repo_root = self
            .app
            .cas_dir()
            .parent()
            .unwrap_or(self.app.cas_dir())
            .to_path_buf();
        let request = self.app.delivery_request(&[]);
        self.queue_jobs.submit_inbox_retraction(
            "supervisor",
            teams.inbox_pruner(),
            repo_root,
            Box::new(move |stores| request.read(stores).into_data()),
        );
    }
}

impl FactoryDaemon {
    /// One loop pass's share of the two-second director refresh (cas-ee9ab).
    /// Returns true when a director read was applied.
    pub(super) async fn director_refresh_pass(
        &mut self,
        refresh: &mut super::director_refresh::DirectorRefresh,
        due: bool,
        store_worker: &super::store_worker::StoreWorker,
        loop_progress: &super::loop_watchdog::LoopProgress,
    ) -> bool {
        self.director_refresh_pass_with(refresh, due, store_worker, loop_progress, 0.0)
            .await
    }

    async fn director_refresh_pass_with(
        &mut self,
        refresh: &mut super::director_refresh::DirectorRefresh,
        due: bool,
        store_worker: &super::store_worker::StoreWorker,
        loop_progress: &super::loop_watchdog::LoopProgress,
        tick_interval_ms: f64,
    ) -> bool {
        // Dynamic spawns and recycles also change the durable session roster,
        // even when the director has no lifecycle events to deliver.
        self.flush_epic_focus_write(store_worker);
        loop_progress.enter(super::loop_watchdog::LoopPhase::RefreshDirector);
        let refresh_started = std::time::Instant::now();
        let cas_dir = self.app.cas_dir().to_path_buf();
        for completed in refresh.take_merge_alert_inbox(&mut self.queue_jobs) {
            self.finish_merge_alert_inbox(completed).await;
        }
        self.relay_jobs.submit_external_work_reconciliation(
            &mut self.queue_jobs,
            move || super::external_work_reconcile_jobs::reconcile_due(&cas_dir),
        );
        // cas-223f: health probes that finished since the last pass feed this
        // pass's stall confirmation.
        let (reports, failed) = refresh.take_health_reports(&mut self.queue_jobs);
        for report in reports {
            self.app.apply_worker_health_report(report);
        }
        for (worker, task) in failed {
            self.app.clear_worker_health_probe(&worker, &task);
        }
        // cas-223f review M1: the git half of the stall evidence arrives from
        // its own job on the external lane.
        if let Some(evidence) = refresh.take_ci_status(&mut self.queue_jobs) {
            self.app.apply_ci_status(evidence);
        }
        let mut refreshed = false;
        // cas-4491f review L1: a finished snapshot is applied before a due one
        // starts, so a pending result does not defer the due one into a
        // second read. Its delivery read, if any, still goes first (the
        // start below waits for it).
        if let Some(load) = refresh.take_director_read(&mut self.queue_jobs) {
            refreshed = true;
            let applied = crate::ui::factory::app::timed_refresh_step("apply director read", || self.app.apply_director_refresh(load));
            // cas-3b16: a changed supervisor-stall tracker is written to the
            // session metadata on the store worker, not on the loop.
            if let Some((path, tracker)) = self.app.take_supervisor_stall_write() {
                store_worker.submit("supervisor stall metric", move || {
                    if let Err(error) =
                        crate::ui::factory::app::persist_session_metadata_supervisor_stall_at(&path, &tracker)
                    {
                        tracing::warn!(%error, "failed to persist supervisor stall metric");
                    }
                });
            }
            // cas-4491f: phantom worker panes' teardown requests are queued on
            // the store worker too.
            if let Some((cas_dir, session, workers)) = self.app.take_phantom_teardowns() {
                store_worker.submit("phantom worker teardown", move || {
                    crate::ui::factory::app::queue_phantom_worker_teardowns(&cas_dir, &session, &workers);
                });
            }
            let requests = self.app.take_worker_health_requests();
            for (worker, task) in refresh.submit_health_probes(&self.app, &mut self.queue_jobs, requests) {
                self.app.clear_worker_health_probe(&worker, &task);
            }
            refresh.start_ci_status(&self.app, &mut self.queue_jobs);
            match applied {
                // cas-627f: an idle tick with zero events never reads the
                // store again; the relays and follow-up still run.
                Ok(events) if events.is_empty() => {
                    self.deliver_director_events(
                        refresh,
                        events,
                        (Vec::new(), Vec::new(), None),
                        refresh_started,
                        tick_interval_ms,
                        store_worker,
                        loop_progress,
                    )
                    .await;
                }
                Ok(events) => {
                    if let Err(events) = refresh.start_delivery_read(&self.app, &mut self.queue_jobs, events) {
                        self.deliver_without_jobs(refresh, events, refresh_started, tick_interval_ms, store_worker, loop_progress)
                            .await;
                    }
                }
                Err(error) => {
                    tracing::debug!(%error, "director refresh read failed; panels keep the last snapshot");
                }
            }
        }
        // cas-4491f (C2b review L4): a refresh deferred behind a busy read
        // or liveness job starts on the first pass they are done.
        if due || refresh.refresh_requested() {
            crate::ui::factory::app::timed_refresh_step("start director read", || refresh.start_director_read(&self.app, &mut self.queue_jobs));
        }
        let mut delivery = refresh.take_delivery_read(&mut self.queue_jobs);
        // cas-223f review round 2, M3: a read without fresh data is retried
        // before its events are delivered unrevalidated; never revalidated
        // against the cache.
        if let Some((events, inputs)) = delivery.take() {
            if inputs.is_fresh() {
                refresh.note_fresh_delivery_read();
                delivery = Some((events, inputs));
            } else if let Some(events) = refresh.retry_stale_delivery(events) {
                delivery = Some((events, inputs));
            }
        }
        if let Some((events, inputs)) = delivery {
            // cas-627f: one unfiltered read, taken after detection, shared by
            // revalidation and prompt generation. See
            // `revalidate_and_prompt_for_delivery` doc comment.
            loop_progress.enter(super::loop_watchdog::LoopPhase::RefreshDelivery);
            let delivery = self
                .app
                .revalidate_and_prompt_for_delivery_from(&events, inputs);
            self.deliver_director_events(
                refresh,
                events,
                delivery,
                refresh_started,
                tick_interval_ms,
                store_worker,
                loop_progress,
            )
            .await;
        }
        // Events waiting for a delivery read (detected while one ran, or
        // re-queued after one failed) start on any pass where the slot is
        // free, not only after a successful read (review round 2, M2).
        if let Err(events) = refresh.start_delivery_backlog(&self.app, &mut self.queue_jobs) {
            self.deliver_without_jobs(refresh, events, refresh_started, tick_interval_ms, store_worker, loop_progress)
                .await;
        }
        // cas-223f: prompts whose fresh re-check (`RevalidateFresh`) finished.
        for batch in refresh.take_deliverability(&mut self.queue_jobs) {
            loop_progress.enter(super::loop_watchdog::LoopPhase::RefreshInject);
            self.inject_rechecked_batch(batch, store_worker).await;
        }
        refreshed
    }

    /// Last resort when the job runtime could not start its lanes: revalidate
    /// against a read on this pass, as before cas-ee9ab.
    async fn deliver_without_jobs(
        &mut self,
        refresh: &mut super::director_refresh::DirectorRefresh,
        events: Vec<crate::ui::factory::director::DirectorEvent>,
        refresh_started: std::time::Instant,
        tick_interval_ms: f64,
        store_worker: &super::store_worker::StoreWorker,
        loop_progress: &super::loop_watchdog::LoopProgress,
    ) {
        tracing::warn!("factory job runtime unavailable; revalidating on the loop");
        loop_progress.enter(super::loop_watchdog::LoopPhase::RefreshDelivery);
        let delivery = self.app.revalidate_and_prompt_for_delivery(&events);
        self.deliver_director_events(
            refresh,
            events,
            delivery,
            refresh_started,
            tick_interval_ms,
            store_worker,
            loop_progress,
        )
        .await;
    }

    /// Record, relay and deliver one batch of revalidated director events,
    /// and run the refresh follow-up (epic transitions, reminders, cloud
    /// state, inbox retraction). Runs once per applied director read.
    #[allow(clippy::type_complexity, clippy::too_many_arguments)]
    pub(super) async fn deliver_director_events(
        &mut self,
        refresh: &mut super::director_refresh::DirectorRefresh,
        events: Vec<crate::ui::factory::director::DirectorEvent>,
        delivery: (
            Vec<crate::ui::factory::director::DirectorEvent>,
            Vec<crate::ui::factory::director::Prompt>,
            Option<crate::ui::factory::director::DirectorData>,
        ),
        refresh_started: std::time::Instant,
        tick_interval_ms: f64,
        store_worker: &super::store_worker::StoreWorker,
        loop_progress: &super::loop_watchdog::LoopProgress,
    ) {
        let (delivery_events, prompts, _unfiltered_data) = delivery;
        tracing::debug!(
            target: "cas::coordination",
            stage = "director_refresh",
            channel = "director_events",
            event_count = delivery_events.len(),
            stale_event_count = events.len().saturating_sub(delivery_events.len()),
            prompt_count = prompts.len(),
            tick_interval_ms,
            "director refresh tick processed"
        );

        // Record events for export
        loop_progress.enter(super::loop_watchdog::LoopPhase::RefreshRelays);
        crate::ui::factory::app::timed_refresh_step("record events", || self.app.record_events(&delivery_events));
        if !delivery_events.is_empty() {
            self.session_summarizer.note_semantic_event();
        }

        // Send notifications for detected events
        crate::ui::factory::app::timed_refresh_step("notify events", || self.app.notify_events(&delivery_events));
        crate::ui::factory::app::timed_refresh_step("usage-limit relay", || self.relay_usage_limited_workers());
        // cas-8a55: an account failure kills the worker's first
        // turn while its process keeps heartbeating, so it has to
        // be read from the transcript on the same tick that reads
        // availability rather than waiting for a stall threshold.
        crate::ui::factory::app::timed_refresh_step("auth-failure relay", || self.relay_auth_failed_workers());
        // cas-4143: answer (or surface) teammate permission
        // requests Claude parked for a lead nobody plays.
        crate::ui::factory::app::timed_refresh_step("permission relay", || self.relay_worker_permission_requests());
        // cas-2ffe: simultaneous harness exits are one incident.
        crate::ui::factory::app::timed_refresh_step("correlated deaths relay", || self.relay_correlated_worker_deaths());

        // cas-d4ae: the detector has already emitted exactly one
        // event for this idle/stall episode and the app just
        // revalidated it against current task/worker state. Send
        // the actionable cases through the durable supervisor
        // wake lane before ordinary prompt injection.
        // GH #1165: on the store worker, in event order.
        if !delivery_events.is_empty() {
            let cas_dir = self.app.cas_dir().to_path_buf();
            let events = delivery_events
                .iter()
                .map(|event| (event.clone(), self.app.worker_attention_episode_start(event)))
                .collect::<Vec<_>>();
            store_worker.submit("worker attention relays", move || {
                for (event, episode_start) in &events {
                    enqueue_worker_attention_relay_for_episode(&cas_dir, event, *episode_start);
                }
            });
        }

        // Handle epic state transitions
        loop_progress.enter(super::loop_watchdog::LoopPhase::RefreshFollowUp);
        let changes = crate::ui::factory::app::timed_refresh_step("epic events", || self.app.handle_epic_events(&delivery_events));
        // cas-5e72: a focus change's metadata write goes to the store worker,
        // which runs writes in order, so the last focus wins.
        self.flush_epic_focus_write(store_worker);
        for change in changes {
            let _ = self.handle_epic_change(change).await;
        }
        self.flush_epic_focus_write(store_worker);

        // Process reminders (time-based and event-based)
        crate::ui::factory::app::timed_refresh_step("reminders", || self.process_reminders(&delivery_events));

        // Push state and events to cloud (best-effort, no-op if not connected)
        crate::ui::factory::app::timed_refresh_step("cloud push", || {
            self.push_cloud_events(&delivery_events);
            self.push_cloud_state();
        });

        // cas-ed6c: retract stale WorkerIdle-class alerts already
        // queued in the supervisor's inbox — before injecting any
        // NEW prompts this tick — using the SAME live snapshot
        // just loaded for revalidation (no extra DB load). A
        // `WorkerIdle` alert is revalidated against live state
        // only at the instant it's written; if the named worker
        // gained a real assignment before the recipient's next
        // turn boundary (Claude Code only polls its inbox then,
        // and `read` is never flipped by production code — see
        // `InboxMessage::retract_worker` doc), the written row
        // just sits there, stale, with nothing to catch it. This
        // is the live-evidence-quoted-a-superseded-tip class of
        // bug (three workers announced idle/ready ~7 minutes
        // after each had a genuine InProgress assignment).
        //
        // cas-d2a2 (factory loop v2 §4.3): the sweep is a job. The decision
        // comes from a fresh read issued for it (RevalidateFresh, on the
        // queue jobs' read connection), never the cached director snapshot
        // (app/mod.rs:1541–1547). The merge-freshness checks (cas-e48f re-reads
        // the CURRENT epic tip) and the epic-completion check (cas-06ca) then
        // run on the blocking lane, before the inbox lock is taken; missing
        // state preserves every row.
        self.submit_supervisor_inbox_retraction();

        // Inject prompts (config already checked in generate_prompt)
        loop_progress.enter(super::loop_watchdog::LoopPhase::RefreshInject);
        // cas-223f: a prompt that must be re-checked right before transport
        // (a retractable alert, or a taskless-worker alert that drops once the
        // worker is assigned) waits for a fresh read as a `RevalidateFresh`
        // job, after this tick's epic merges. The whole batch waits with it
        // so prompts keep their order. cas-3b16 (C2 review round 1, L7): so
        // does a later batch while an earlier one still waits; re-checks
        // share one key and finish in submission order, and a batch with
        // nothing to re-check reads nothing.
        if !prompts.is_empty()
            && (refresh.has_pending_deliverability()
                || prompts
                    .iter()
                    .any(crate::ui::factory::app::prompt_needs_inject_read))
        {
            match refresh.start_deliverability_check(
                &self.app,
                &mut self.queue_jobs,
                prompts,
                delivery_events.clone(),
                refresh_started,
            ) {
                Ok(()) => return,
                Err(prompts) => {
                    // No job lanes: re-check on this pass, as before.
                    for mut prompt in prompts {
                        if !self.app.refresh_prompt_for_delivery(&mut prompt) {
                            log_dropped_stale_prompt(&prompt);
                            continue;
                        }
                        self.inject_director_prompt(prompt, &delivery_events, refresh_started, store_worker, None)
                            .await;
                    }
                    return;
                }
            }
        }
        // cas-4491f: nothing here needs a read; an assignment wake went
        // through the re-check above.
        for prompt in prompts {
            self.inject_director_prompt(
                prompt,
                &delivery_events,
                refresh_started,
                store_worker,
                Some(crate::ui::factory::app::AssignmentRead::NotAssignment),
            )
            .await;
        }
    }

    /// Inject a re-checked prompt batch, in order: a prompt the fresh read
    /// retracted is dropped, and an assignment wake uses the task state the
    /// job read with it (cas-4491f).
    pub(super) async fn inject_rechecked_batch(
        &mut self,
        batch: super::factory_jobs::DeliverabilityBatch,
        store_worker: &super::store_worker::StoreWorker,
    ) {
        let super::factory_jobs::DeliverabilityBatch {
            prompts,
            assignments,
            events,
            refresh_started,
        } = batch;
        let mut assignments = assignments.into_iter();
        for (prompt, deliverable) in prompts {
            let assignment = assignments
                .next()
                .unwrap_or(crate::ui::factory::app::AssignmentRead::Unread);
            if !deliverable {
                log_dropped_stale_prompt(&prompt);
                continue;
            }
            self.inject_director_prompt(prompt, &events, refresh_started, store_worker, Some(assignment))
                .await;
        }
    }

    /// Keep the result owner alive through native completions and fallback
    /// rechecks before closing the external lane and draining store writes.
    pub(super) async fn drain_director_deliveries_for_shutdown(
        &mut self,
        refresh: &mut super::director_refresh::DirectorRefresh,
        store_worker: &super::store_worker::StoreWorker,
        timeout: Duration,
    ) {
        let (rechecked, unchecked) = refresh
            .drain_deliverability(&mut self.queue_jobs, timeout.min(Duration::from_secs(2)))
            .await;
        for batch in rechecked {
            self.inject_rechecked_batch(batch, store_worker).await;
        }
        for pending in unchecked {
            for mut prompt in pending.prompts {
                if !self.app.refresh_prompt_for_delivery(&mut prompt) {
                    log_dropped_stale_prompt(&prompt);
                    continue;
                }
                self.inject_director_prompt(prompt, &pending.events, pending.refresh_started, store_worker, None)
                    .await;
            }
        }
        let deadline = Instant::now() + timeout;
        loop {
            for completed in refresh.take_merge_alert_inbox(&mut self.queue_jobs) {
                self.finish_merge_alert_inbox(completed).await;
            }
            if !self.queue_jobs.external_kind_pending(super::factory_jobs::MERGE_ALERT_INBOX) {
                break;
            }
            if Instant::now() >= deadline {
                tracing::warn!("merge alert jobs still pending at shutdown deadline");
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    }

    /// Shutdown: hand off the last deferred session-metadata write, then let
    /// every queued store write land, each step bounded by `timeout` so a
    /// wedged write cannot hang shutdown (cas-54a2). Returns whether the
    /// store worker went idle.
    pub(super) fn finish_shutdown_writes(
        &mut self,
        store_worker: &super::store_worker::StoreWorker,
        timeout: Duration,
    ) -> bool {
        // cas-54a2: a focus change made since the last refresh pass (from the
        // keyboard, say) is still in its slot; it must not be lost.
        self.flush_epic_focus_write(store_worker);
        // cas-d2a2: queue stamps run on the StoreWorker through the queue
        // job lanes; drain the lanes first so no critical stamp is still
        // waiting on a lane while the StoreWorker reports idle (jobs.rs).
        let pointers_idle = self.finish_pointer_jobs_for_shutdown(timeout);
        let queue_cas_dir = self.app.cas_dir().to_path_buf();
        if !self.queue_jobs.drain_for_shutdown(&queue_cas_dir, timeout) {
            tracing::warn!("prompt-queue job lanes still busy at shutdown");
        }
        // Let queued store writes (the epic focus and worker-hold clears
        // among them) land before cleanup tears the session down.
        let idle = store_worker.wait_idle(timeout);
        if !idle {
            tracing::warn!(
                pending = store_worker.pending(),
                "factory store worker still busy at shutdown"
            );
        }
        idle && pointers_idle
    }

    /// Hand the latest epic-focus write to the store worker (cas-5e72).
    pub(super) fn flush_epic_focus_write(&mut self, store_worker: &super::store_worker::StoreWorker) {
        if let Some((path, workers)) = self.app.take_worker_roster_write() {
            store_worker.submit("worker roster metadata", move || {
                if let Err(error) = crate::ui::factory::app::update_session_metadata_at(&path, |metadata| {
                    metadata.workers = workers;
                }) {
                    tracing::warn!(%error, "failed to persist worker roster");
                }
            });
        }
        if let Some((path, epic_id)) = self.app.take_epic_focus_write() {
            store_worker.submit("epic focus metadata", move || {
                crate::ui::factory::app::persist_epic_focus_at(&path, epic_id.as_deref());
            });
        }
    }

    /// Route one revalidated director prompt to its recipient: a stall nudge
    /// to the durable queue, an assignment through the assignment row, an
    /// unready pane through the queue, and everything else straight to the
    /// recipient's inbox or PTY.
    ///
    /// `assignment` is the assignment state read off the loop with the
    /// prompt's re-check (cas-4491f). `None` reads it here: only the paths
    /// that run without job lanes (no runtime, or shutdown) pass `None`.
    async fn inject_director_prompt(
        &mut self,
        prompt: crate::ui::factory::director::Prompt,
        delivery_events: &[crate::ui::factory::director::DirectorEvent],
        refresh_started: std::time::Instant,
        store_worker: &super::store_worker::StoreWorker,
        assignment: Option<crate::ui::factory::app::AssignmentRead>,
    ) {
        use crate::ui::factory::app::AssignmentRead;
        // A green delivery has one durable reservation shared with later park
        // and manual merge requests. Never bypass that queue via direct PTY or
        // native inbox transport. Red/unknown decision prompts keep their lane.
        if prompt.is_merge_ready() {
            let cas_dir = self.app.cas_dir().to_path_buf();
            let session = self.session_name.clone();
            store_worker.submit("director merge readiness", move || {
                if let Err(error) = super::delivery::enqueue_merge_ready_prompt(&cas_dir, &session, &prompt) {
                    tracing::warn!(%error, "director merge readiness enqueue failed");
                }
            });
            return;
        }
        // A routine nudge has one queryable row and uses the queue's
        // busy-recipient gate. Never write it directly into a Teams inbox.
        if let Some(task_id) = prompt.stall_nudge_task.clone() {
            let event = delivery_events.iter().find(|event| matches!(event,
                crate::ui::factory::director::DirectorEvent::WorkerStalled {
                    worker, task_id: event_task, escalate: false, ..
                } if worker == &prompt.target && event_task == &task_id));
            let episode_start = event.and_then(|event| self.app.worker_attention_episode_start(event));
            let cas_dir = self.app.cas_dir().to_path_buf();
            let session = self.session_name.clone();
            store_worker.submit("worker stall nudge", move || {
                if let Err(error) = enqueue_stall_nudge_prompt(&cas_dir, &session, &prompt, episode_start) {
                    tracing::warn!(%error, "worker stall nudge enqueue failed");
                }
            });
            return;
        }
        // GH #682: the event snapshot was read before this
        // loop. Assignment state is re-read at the final direct
        // transport boundary so a task that moved beyond Open
        // in that gap cannot receive stale `task start`
        // boilerplate. cas-4491f: the daemon reads it in the
        // prompt's `RevalidateFresh` job, not here.
        let assignment = assignment
            .unwrap_or_else(|| AssignmentRead::read(self.app.cas_dir(), None, &prompt));
        match assignment {
            AssignmentRead::Stale(task_id, status) => {
                tracing::info!(
                    target: "cas::coordination",
                    stage = "suppress_terminal_assignment",
                    target_agent = %prompt.target,
                    task_id = %task_id,
                    status = %status,
                    "cas-2b0b: suppressed a direct assignment for a terminal task"
                );
                return;
            }
            // Task mutations and director snapshots share one durable
            // assignment row; a late tick cannot duplicate the wake. The row
            // is written on the store worker (cas-4491f). If that write fails
            // the wake goes to the durable queue rather than being lost.
            AssignmentRead::Task(task) => {
                let cas_dir = self.app.cas_dir().to_path_buf();
                let session = self.session_name.clone();
                store_worker.submit("director assignment wake", move || {
                    if let Err(error) = crate::task_assignment::enqueue(&cas_dir, &task) {
                        tracing::warn!(%error, "assignment enqueue failed; queueing the director wake instead");
                        if let Err(error) = super::delivery::enqueue_director_prompt(
                            &cas_dir,
                            &session,
                            &prompt.target,
                            &prompt.text,
                        ) {
                            tracing::warn!(
                                target: "cas::coordination",
                                stage = "director_prompt_queue_failed",
                                target_agent = %prompt.target,
                                error = %error,
                                "assignment wake could not be queued; it may be lost"
                            );
                        }
                    }
                });
                return;
            }
            // Review M4 (GH #682): the re-check never read this wake's task.
            // Read it, check it and write its row on the store worker; a
            // wake that is not an assignment after all is delivered as usual.
            AssignmentRead::Unread
                if crate::prompt_revalidation::assignment_solicited_task_id(&prompt.text)
                    .is_some() =>
            {
                let cas_dir = self.app.cas_dir().to_path_buf();
                let session = self.session_name.clone();
                store_worker.submit("director assignment wake (unread)", move || {
                    crate::ui::factory::app::resolve_unread_assignment(&cas_dir, &session, &prompt);
                });
                return;
            }
            AssignmentRead::NotAssignment | AssignmentRead::Unavailable | AssignmentRead::Unread => {}
        }
        // cas-ae6d (GH #100): a loss-intolerant prompt (today:
        // the assignment wake-up) bound for a PTY pane that is
        // not ready for injection goes to the durable
        // prompt_queue instead of being written into a pane
        // that silently swallows it during harness startup.
        // The director lane has no readiness gate and no
        // retry; the queue lane has both. Claude-under-teams
        // recipients are unaffected — their inbox write is
        // durable by construction, which is exactly why the
        // same assignment woke a claude worker while codex
        // workers were left idle.
        //
        // cas-4491f: the queue write runs on the store worker. A write that
        // fails there is logged; injecting into a pane that is not ready
        // would only be swallowed by its startup input buffer.
        if crate::supervisor_message_wakes::quiet(&prompt.text)
            || self.route_director_prompt_to_queue(&prompt)
        {
            self.submit_director_prompt_enqueue(store_worker, &prompt, "director_prompt_queued");
            return;
        }
        if self.submit_merge_alert_inbox(&prompt, refresh_started) {
            return;
        }
        // cas-f9e8 telemetry: measure director prompt
        // injection latency from the start of this refresh
        // tick to the completion of the inbox write. This
        // is Channel C's send→deliver envelope and is the
        // number that tells us whether refresh_interval
        // needs to be lowered for the P99 SLO.
        let inject_started = std::time::Instant::now();
        // Recipient-aware routing (cas-b68a): a director event aimed
        // at a Codex agent must reach its PTY, not a Claude inbox.
        let inject_result = self
            .deliver_to_worker(
                &prompt.target,
                super::teams::DIRECTOR_AGENT_NAME,
                &prompt.text,
                None,
                // D-4 (cas-405f): pass the director's config.json color so
                // the inbox bubble matches the registered team entry.
                Some(super::teams::DIRECTOR_AGENT_COLOR),
                // cas-ed6c: tag WorkerIdle-class alerts so a
                // later sweep can retract them if the named
                // worker gains a real assignment before the
                // recipient ever reads the queued row.
                prompt.retract_worker.as_deref(),
                // cas-e48f: tag MERGE REQUIRED alerts so a
                // later sweep can retract them if this task's
                // merge lands (or it leaves AwaitingMerge)
                // before the recipient ever reads the row.
                prompt.retract_task.as_deref(),
                // cas-06ca: carry the epic occurrence identity
                // through transport so unread completion rows
                // can be retracted if live state advances.
                prompt.retract_epic.as_deref(),
            )
            .await;
        // cas-ae6d: a durable prompt that did not actually land
        // (transport error, or a composer-dirty deferral this
        // lane cannot retry) is re-queued rather than lost. An
        // unready pane never reaches this direct inject
        // (cas-4491f: its queue write is not retried here).
        let delivered = super::delivery::durable_delivery_landed(
            matches!(inject_result, Ok(cas_mux::InjectOutcome::Delivered)),
            false,
        );
        if super::delivery::needs_durable_followup(delivered, prompt.durable_retry)
        {
            self.submit_director_prompt_enqueue(store_worker, &prompt, "director_prompt_requeued");
        }
        super::delivery::log_director_delivery(&prompt, &inject_result, inject_started, refresh_started);
    }
}

/// A prompt its fresh re-check found stale.
fn log_dropped_stale_prompt(prompt: &crate::ui::factory::director::Prompt) {
    tracing::info!(
        target: "cas::coordination",
        stage = "drop_stale_taskless_worker_before_injection",
        worker = prompt.drop_if_worker_assigned.as_deref().unwrap_or(""),
        "dropped taskless-worker alert because assignment landed after batch revalidation"
    );
}

#[cfg(test)]
mod stall_nudge_routing_rework_tests {
    use super::*;

    #[test]
    fn stalled_nudge_lifecycle_routes_missing_episode_durably_and_ignores_other_prompts() {
        let root = tempfile::tempdir().unwrap();
        let cas_dir = crate::store::init_cas_dir(root.path()).unwrap();
        let mut prompt = crate::ui::factory::director::Prompt {
            merge_ready: false, merge_alert: None,
            target: "worker".into(), text: "quiet".into(), stall_nudge_task: Some("cas-unknown".into()),
            retract_worker: None, retract_task: None, retract_epic: None,
            drop_if_worker_assigned: None, durable_retry: true,
        };
        let first = enqueue_stall_nudge_prompt(&cas_dir, "session", &prompt, None).unwrap().unwrap();
        assert_eq!(enqueue_stall_nudge_prompt(&cas_dir, "session", &prompt, None).unwrap(), Some(first));
        let queue = crate::store::open_prompt_queue_store(&cas_dir).unwrap();
        assert_eq!(queue.queued_prompt(first).unwrap().unwrap().kind.as_deref(), Some("worker_stall_nudge"));
        prompt.stall_nudge_task = None;
        assert_eq!(enqueue_stall_nudge_prompt(&cas_dir, "session", &prompt, Some(chrono::Utc::now())).unwrap(), None);
        assert_eq!(queue.peek_all(10).unwrap().len(), 1);
    }
}


async fn persist_communication_session_summary(cas_dir: std::path::PathBuf, session: String) {
    match tokio::task::spawn_blocking(move || {
        cas_store::communication_slo::write_session_end(&cas_dir, &session, chrono::Utc::now())
    }).await {
        Ok(Ok(summary)) => tracing::info!(summary = %summary.message(), "factory communication summary"),
        Ok(Err(error)) => tracing::warn!(%error, "factory communication summary unavailable"),
        Err(error) => tracing::warn!(%error, "factory communication summary job failed"),
    }
}

#[cfg(test)]
mod communication_summary_tests {
    #[tokio::test]
    async fn cas_7e24_factory_session_end_persists_summary() {
        let project = tempfile::tempdir().unwrap();
        let root = crate::store::init_cas_dir(project.path()).unwrap();
        use cas_store::PromptQueueStore;
        let queue = cas_store::SqlitePromptQueueStore::open(&root).unwrap();
        queue.init().unwrap();
        queue.enqueue_with_session("supervisor", "worker", "body", "ended-session").unwrap();
        super::persist_communication_session_summary(root.clone(), "ended-session".into()).await;
        let conn = rusqlite::Connection::open(root.join("cas.db")).unwrap();
        let summary: String = conn.query_row("SELECT summary_json FROM communication_slo_summaries WHERE factory_session='ended-session'", [], |r| r.get(0)).unwrap();
        let summary: cas_store::communication_slo::CommunicationSlos = serde_json::from_str(&summary).unwrap();
        assert_eq!(summary.lines().len(), 5);
        assert_eq!(summary.factory_session, "ended-session");
    }
}
