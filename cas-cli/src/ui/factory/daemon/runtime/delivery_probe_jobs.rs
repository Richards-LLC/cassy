//! Message-bound watchdog evidence on the shared relay job lanes (cas-031e).
use super::queue_and_events::message_attention::{AttentionInputs, MessageAttention};
use super::queue_and_events::{
    IDLE_UNACKED_RELAY_AFTER, NormalDeliveryProbe, NormalDeliveryTurnEvidence, RecipientActivity,
    classify_normal_delivery_turn, normal_delivery_watchdog_flag_detail,
};
use crate::store::open_agent_store;
use chrono::{DateTime, Utc};
use std::collections::HashMap;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

pub(crate) struct NormalPrepared {
    pub(crate) row_id: i64,
    pub(crate) probe: NormalDeliveryProbe,
    pub(crate) activity: RecipientActivity,
    pub(crate) evidence: NormalDeliveryTurnEvidence,
    pub(crate) live: Arc<AtomicBool>,
}
#[derive(Default)]
pub(crate) struct NormalProbeJobs {
    owned: HashMap<i64, (NormalDeliveryProbe, Arc<AtomicBool>)>,
    pub(crate) ready: HashMap<i64, NormalPrepared>,
}
impl NormalProbeJobs {
    pub(crate) fn sync_current(&mut self, current: &HashMap<i64, NormalDeliveryProbe>) {
        let stale: Vec<_> = self
            .owned
            .iter()
            .filter(|(id, (probe, _))| current.get(id) != Some(probe))
            .map(|(id, _)| *id)
            .collect();
        for id in stale {
            self.forget_row(id);
        }
        self.ready
            .retain(|id, result| current.get(id) == Some(&result.probe));
    }
    pub(crate) fn track(&mut self, id: i64, probe: &NormalDeliveryProbe) -> Arc<AtomicBool> {
        self.owned
            .entry(id)
            .or_insert_with(|| (probe.clone(), Arc::new(AtomicBool::new(true))))
            .1
            .clone()
    }
    pub(crate) fn forget_row(&mut self, id: i64) {
        if let Some((_, live)) = self.owned.remove(&id) {
            live.store(false, Ordering::Release);
        }
        self.ready.remove(&id);
    }
    pub(crate) fn forget_pane(&mut self, pane: &str) {
        let ids: Vec<_> = self
            .owned
            .iter()
            .filter(|(_, (probe, _))| probe.pane == pane || probe.target == pane)
            .map(|(id, _)| *id)
            .collect();
        for id in ids {
            self.forget_row(id);
        }
    }
}
pub(crate) fn read_normal_probe(
    cas_dir: &std::path::Path,
    session: &str,
    row_id: i64,
    probe: NormalDeliveryProbe,
    agent_id: Option<String>,
    live: Arc<AtomicBool>,
) -> NormalPrepared {
    let activity = crate::store::open_prompt_queue_store(cas_dir)
        .map(|queue| {
            authored_activity_io(
                cas_dir,
                session,
                agent_id,
                queue.as_ref(),
                &probe.pane,
                probe.delivered_at_utc,
            )
        })
        .unwrap_or(RecipientActivity::Unknown);
    let mut evidence = NormalDeliveryTurnEvidence::Unavailable {
        pane_output_grew: false,
    };
    if activity == RecipientActivity::Absent {
        evidence = normal_delivery_turn_evidence_io(
            cas_dir,
            &probe.pane,
            probe.delivered_at_utc,
            &probe.prompt,
            false,
        );
        if evidence != NormalDeliveryTurnEvidence::TurnStarted
            && crate::ui::factory::director::idle_worker_liveness(cas_dir, &probe.pane).is_some_and(
                |o| o.state == crate::mcp::tools::service::worker_liveness::Liveness::Executing,
            )
        {
            evidence = NormalDeliveryTurnEvidence::RecipientBusy;
        }
    }
    NormalPrepared {
        row_id,
        probe,
        activity,
        evidence,
        live,
    }
}
pub(crate) fn write_normal_flag(
    cas_dir: &std::path::Path,
    session: &str,
    row_id: i64,
    probe: &NormalDeliveryProbe,
    evidence: NormalDeliveryTurnEvidence,
    live: &Arc<AtomicBool>,
) -> bool {
    if !live.load(Ordering::Acquire) {
        return false;
    }
    let Ok(queue) = crate::store::open_prompt_queue_store(cas_dir) else {
        return false;
    };
    let Ok(current) = queue.queued_prompt(row_id) else {
        return false;
    };
    // An acknowledged, removed or replaced original needs no health incident.
    let Some(row) = current else {
        return true;
    };
    if row.prompt != probe.prompt
        || row.target != probe.target
        || !super::poison_sweep_jobs::still_unread(queue.as_ref(), &row)
    {
        return true;
    }
    let detail = normal_delivery_watchdog_flag_detail(evidence);
    if let Err(error) =
        queue.record_wake_attempt(row_id, cas_store::WakeAttempt::Failed, Some(&detail))
    {
        tracing::warn!(row_id, %error, "failed to record delivery watchdog flag");
    }
    let persisted = matches!(
        super::lifecycle::enqueue_worker_delivery_stalled_relay(
            cas_dir,
            &probe.pane,
            row_id,
            Utc::now()
        ),
        super::lifecycle::WorkerAttentionRelayOutcome::Persisted { .. }
    );
    if persisted {
        tracing::warn!(target: "cas::coordination", stage = "normal_delivery_watchdog_flagged",
            message_id = row_id, target_agent = %probe.pane, ?evidence,
            "normal delivery started no harness turn after retry; supervisor notified without urgent escalation");
    }
    persisted
}
pub(crate) struct IdleCandidate {
    pub(crate) row: cas_store::QueuedPrompt,
    pub(crate) completion: Option<DateTime<Utc>>,
}
pub(crate) fn read_idle_unread(context: &AttentionInputs, supervisor: &str) -> Vec<IdleCandidate> {
    let Ok(queue) = crate::store::open_prompt_queue_store(&context.cas_dir) else {
        return vec![];
    };
    let after = i64::try_from(IDLE_UNACKED_RELAY_AFTER.as_secs()).unwrap_or(i64::MAX);
    let rows = match queue.delivery_stalled_candidates(&context.session_name, after, after, 50) {
        Ok(rows) => rows,
        Err(error) => {
            tracing::warn!(%error, "cas-913c: failed to scan idle unacked messages");
            return vec![];
        }
    };
    let decisions = context.decisions(queue.as_ref(), &rows);
    rows.into_iter()
        .filter(|row| {
            decisions.get(&row.id) == Some(&MessageAttention::Needed)
                && row.target != "supervisor"
                && !row.target.eq_ignore_ascii_case(supervisor)
        })
        .map(|row| {
            let completion =
                recipient_turn_completion_after_io(&context.cas_dir, &row.target, row.created_at);
            IdleCandidate { row, completion }
        })
        .collect()
}
pub(crate) fn validate_idle_unread(
    context: &AttentionInputs,
    candidate: IdleCandidate,
) -> Option<IdleCandidate> {
    let Ok(queue) = crate::store::open_prompt_queue_store(&context.cas_dir) else {
        return None;
    };
    let old = &candidate.row;
    let Ok(Some(row)) = queue.queued_prompt(old.id) else {
        return None;
    };
    if row.created_at != old.created_at
        || row.target != old.target
        || row.prompt != old.prompt
        || row.factory_session != old.factory_session
    {
        return None;
    }
    if context
        .decisions(queue.as_ref(), std::slice::from_ref(&row))
        .get(&row.id)
        != Some(&MessageAttention::Needed)
    {
        return None;
    }
    Some(candidate)
}

pub(crate) fn write_idle_unread(
    context: &AttentionInputs,
    supervisor: &str,
    candidate: IdleCandidate,
    live: &Arc<AtomicBool>,
) -> bool {
    if !live.load(Ordering::Acquire) {
        return false;
    }
    let Ok(queue) = crate::store::open_prompt_queue_store(&context.cas_dir) else {
        return false;
    };
    let row = candidate.row;
    if !super::poison_sweep_jobs::still_unread(queue.as_ref(), &row) {
        return false;
    }
    let summary: String = row
        .summary
        .as_deref()
        .unwrap_or("(no summary)")
        .chars()
        .take(240)
        .collect();
    let idle_since = candidate
        .completion
        .map(|at| at.to_rfc3339())
        .unwrap_or_else(|| "its pane went silent".to_string());
    let notice = format!(
        "<system-notice>Unread message on an idle worker: notification_id={}; target='{}'; from='{}'; summary='{}'. The worker has been idle since {} and has not read or acknowledged it. Check `message_status notification_id={}` and re-send it urgently (`coordination action=interrupt`) if the worker should act on it.</system-notice>",
        row.id, row.target, row.source, summary, idle_since, row.id
    );
    match queue.enqueue_idempotent(
        "daemon",
        supervisor,
        &notice,
        Some(&context.session_name),
        Some(&format!(
            "Unread on idle worker {}: #{}",
            row.target, row.id
        )),
        Some(cas_store::NotificationPriority::High),
        &format!("idle-unacked:{}", row.id),
        None,
    ) {
        Ok(cas_store::EnqueueIdempotentResult::Created(relay_id)) => {
            super::delivery::wake_daemon_after_enqueue(&context.cas_dir);
            tracing::warn!(target: "cas::coordination", stage = "idle_unacked_relayed",
                message_id = row.id, relay_id, target_agent = %row.target,
                "cas-913c: relayed an unread message on an idle worker to the supervisor");
            true
        }
        Ok(cas_store::EnqueueIdempotentResult::AlreadyExists(_)) => true,
        Err(error) => {
            tracing::warn!(message_id=row.id, %error, "cas-913c: failed to relay an unread message on an idle worker");
            false
        }
    }
}

pub(crate) fn authored_activity_io(
    cas_dir: &std::path::Path,
    session: &str,
    snapshot: Option<String>,
    queue: &dyn cas_store::PromptQueueStore,
    worker: &str,
    after: chrono::DateTime<chrono::Utc>,
) -> RecipientActivity {
    match queue.has_message_from_since(worker, session, after) {
        Ok(true) => return RecipientActivity::Found,
        Ok(false) => {}
        Err(error) => {
            tracing::debug!(%error, worker, "recipient reply evidence unavailable this pass");
            return RecipientActivity::Unknown;
        }
    }
    let agent_id = match snapshot {
        Some(id) => id,
        None => match open_agent_store(cas_dir)
            .and_then(|store| store.list(None).map_err(crate::error::CasError::from))
        {
            Ok(agents) => match agents
                .into_iter()
                .filter(|agent| agent.name == worker)
                .max_by_key(|agent| agent.registered_at)
            {
                Some(agent) => agent.id,
                None => return RecipientActivity::Absent,
            },
            Err(error) => {
                tracing::debug!(%error, worker, "recipient agent unavailable this pass");
                return RecipientActivity::Unknown;
            }
        },
    };
    match crate::store::open_event_store(cas_dir).and_then(|store| {
        Ok(store.has_event_since(cas_types::EventType::TaskNoteAdded, &agent_id, after)?)
    }) {
        Ok(true) => RecipientActivity::Found,
        Ok(false) => RecipientActivity::Absent,
        Err(error) => {
            tracing::debug!(%error, worker, "recipient note evidence unavailable this pass");
            RecipientActivity::Unknown
        }
    }
}

pub(crate) fn normal_delivery_turn_evidence_io(
    cas_dir: &std::path::Path,
    pane_target: &str,
    delivered_at: chrono::DateTime<chrono::Utc>,
    prompt: &str,
    pane_output_grew: bool,
) -> NormalDeliveryTurnEvidence {
    let unavailable = NormalDeliveryTurnEvidence::Unavailable { pane_output_grew };
    let Some(agent) = open_agent_store(cas_dir)
        .ok()
        .and_then(|store| store.list(None).ok())
        .and_then(|agents| {
            crate::cli::factory::wedged::preferred_worker_registration(&agents, pane_target)
                .cloned()
        })
    else {
        return unavailable;
    };
    let cli = crate::mcp::tools::service::factory_ops::worker_cli_from_agent(&agent);
    if cli == cas_mux::SupervisorCli::OpenCode {
        return unavailable;
    }
    let Some(path) =
        crate::mcp::tools::service::factory_ops::worker_transcript_path_for_agent(cas_dir, &agent)
            .filter(|path| path.is_file())
    else {
        return unavailable;
    };
    let message = crate::mcp::tools::service::harness_observation::observations_after_delivery(
        &path,
        cli,
        delivered_at,
        prompt,
    );
    let latest =
        crate::mcp::tools::service::harness_observation::latest_turn_observations(&path, cli);
    classify_normal_delivery_turn(
        delivered_at,
        message.wake.map(|observation| observation.at),
        latest.wake.map(|observation| observation.at),
        latest.completion.map(|observation| observation.at),
    )
}

pub(crate) fn recipient_turn_completion_after_io(
    cas_dir: &std::path::Path,
    pane_target: &str,
    after: chrono::DateTime<chrono::Utc>,
) -> Option<chrono::DateTime<chrono::Utc>> {
    let Ok(store) = open_agent_store(cas_dir) else {
        return None;
    };
    let Ok(agents) = store.list(None) else {
        return None;
    };
    let Some(agent) =
        crate::cli::factory::wedged::preferred_worker_registration(&agents, pane_target).cloned()
    else {
        return None;
    };
    let cli = crate::mcp::tools::service::factory_ops::worker_cli_from_agent(&agent);
    let Some(path) =
        crate::mcp::tools::service::factory_ops::worker_transcript_path_for_agent(cas_dir, &agent)
    else {
        return None;
    };
    crate::mcp::tools::service::harness_observation::turn_completion_after(&path, cli, after)
}
