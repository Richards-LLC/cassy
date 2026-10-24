//! C2d: owned spawn ingress and prompt evidence preparation off control.
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::jobs::{Criticality, JobSpec, Lane, Submitted};
use super::prompt_queue_jobs::{ExternalOutcome, QueueJobs};
use super::queue_and_events::append_spawn_audit;

pub(crate) const SPAWN_QUEUE_POLL: &str = "SpawnQueuePoll";
pub(crate) const POLL_CADENCE: Duration = Duration::from_millis(100);

#[derive(Debug, Clone)]
pub(crate) struct SpawnPollRequest {
    pub cas_dir: PathBuf,
    pub session: String,
    pub supervisor: String,
    pub scan_stalls: bool,
    pub reported: HashSet<i64>,
}

#[derive(Debug)]
pub(crate) struct SpawnBatch {
    pub requests: Vec<cas_store::SpawnRequest>,
    pub stalled: Vec<(i64, String)>,
}

/// A fence for the destructive poll: reset/shutdown either takes the finished
/// batch or leaves the running job responsible for cancelling it. The mutex
/// covers only the handoff; no store or filesystem operation runs under it.
#[derive(Default)]
struct SpawnHandoff {
    cancelled: Option<&'static str>,
    result: Option<Result<SpawnBatch, String>>,
}

/// Poll is a dequeue mutation, not a replaceable read. Keep one outstanding
/// command and account for every successful result, even if it arrives late.
#[derive(Default)]
pub(crate) struct SpawnIngress {
    generation: u64,
    in_flight: Option<u64>,
    last_submitted: Option<Instant>,
    handoff: Option<std::sync::Arc<std::sync::Mutex<SpawnHandoff>>>,
    context: Option<SpawnPollRequest>,
    stopped: bool,
    ready: VecDeque<Result<SpawnBatch, String>>,
}

impl SpawnIngress {
    pub(crate) fn cancel(&mut self, jobs: &mut QueueJobs, reason: &'static str, stop: bool) {
        self.stopped = stop;
        self.last_submitted = None;
        let Some(context) = self.context.clone() else {
            return;
        };
        let mut batches: Vec<_> = self.ready.drain(..).collect();
        if let Some(handoff) = &self.handoff {
            let mut handoff = handoff.lock().unwrap();
            handoff.cancelled = Some(reason);
            batches.extend(handoff.result.take());
        }
        if batches.is_empty() {
            return;
        }
        // A completed handoff belongs to control now. Its cancellation writes
        // still use the existing critical writer lane before shutdown closes it.
        let submitted = jobs.submit_external(
            JobSpec {
                kind: SPAWN_QUEUE_POLL,
                key: format!("SpawnQueueCancel:{}:{}", context.session, self.generation),
                lane: Lane::Write,
                criticality: Criticality::Critical,
                generation: self.generation,
                budget: Duration::from_millis(100),
            },
            move || {
                for batch in batches {
                    account_cancelled_spawn_batch(&context, batch, reason);
                }
                Box::new(())
            },
        );
        if !spawn_job_accepted(&submitted) {
            tracing::error!(
                ?submitted,
                reason,
                "spawn cancellation could not be queued before shutdown"
            );
        }
    }

    fn collect(&mut self, jobs: &mut QueueJobs) {
        jobs.apply_completions();
        for result in jobs.take_external_completions_where(|kind| kind == SPAWN_QUEUE_POLL) {
            if !result.key.starts_with("SpawnQueuePoll:") {
                continue;
            }
            if self.in_flight != Some(result.generation) {
                continue;
            }
            self.in_flight = None;
            if let Some(result) = self
                .handoff
                .as_ref()
                .and_then(|state| state.lock().unwrap().result.take())
            {
                self.ready.push_back(result);
            } else if !matches!(result.outcome, Ok(ExternalOutcome::Done(_))) {
                let detail = match result.outcome {
                    Err(error) => format!("critical spawn poll panicked: {}", error.message),
                    _ => "critical spawn poll was dropped".into(),
                };
                self.ready.push_back(Err(detail));
            }
        }
    }

    pub(crate) fn advance(
        &mut self,
        jobs: &mut QueueJobs,
        request: impl FnOnce() -> SpawnPollRequest,
        now: Instant,
    ) -> bool {
        self.collect(jobs);
        if self.stopped
            || self.in_flight.is_some()
            || !self.ready.is_empty()
            || self
                .last_submitted
                .is_some_and(|last| now.saturating_duration_since(last) < POLL_CADENCE)
        {
            return false;
        }
        let request = request();
        self.generation += 1;
        let generation = self.generation;
        self.context = Some(request.clone());
        let state = std::sync::Arc::new(std::sync::Mutex::new(SpawnHandoff::default()));
        self.handoff = Some(state.clone());
        let submitted = jobs.submit_external(
            JobSpec {
                kind: SPAWN_QUEUE_POLL,
                key: format!("SpawnQueuePoll:{}", request.session),
                lane: Lane::Write,
                criticality: Criticality::Critical,
                generation,
                budget: Duration::from_millis(100),
            },
            move || {
                // A queued poll cancelled before execution must leave durable rows
                // queued. A running poll may have consumed rows and must name them.
                if state.lock().unwrap().cancelled.is_some() {
                    return Box::new(());
                }
                let result = poll_spawn_queue(&request);
                if let Err(error) = &result {
                    tracing::error!(%error, "cas-2702: spawn queue poll failed");
                }
                let mut handoff = state.lock().unwrap();
                if let Some(reason) = handoff.cancelled {
                    drop(handoff);
                    account_cancelled_spawn_batch(&request, result, reason);
                } else {
                    handoff.result = Some(result);
                    drop(handoff);
                    super::delivery::wake_daemon_after_enqueue(&request.cas_dir);
                }
                Box::new(())
            },
        );
        let accepted = spawn_job_accepted(&submitted);
        self.last_submitted = Some(now);
        if accepted {
            self.in_flight = Some(generation);
        } else {
            self.ready
                .push_back(Err(format!("spawn poll submission refused: {submitted:?}")));
        }
        self.collect(jobs);
        accepted
    }

    pub(crate) fn take_ready(&mut self) -> Option<Result<SpawnBatch, String>> {
        self.ready.pop_front()
    }
}

fn spawn_job_accepted(submitted: &Submitted) -> bool {
    matches!(
        submitted,
        Submitted::Queued
            | Submitted::QueuedBehindKey
            | Submitted::QueuedDroppedOldest { .. }
            | Submitted::Coalesced { .. }
    )
}

fn account_cancelled_spawn_batch(
    request: &SpawnPollRequest,
    batch: Result<SpawnBatch, String>,
    reason: &str,
) {
    let batch = match batch {
        Ok(batch) => batch,
        Err(error) => {
            tracing::warn!(%error, reason, "cancelled spawn poll failed");
            return;
        }
    };
    if batch.requests.is_empty() {
        return;
    }
    let ids = batch
        .requests
        .iter()
        .map(|row| row.id.to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let detail = format!(
        "{reason}: dropped dequeued spawn requests {ids}; re-issue these requests if still wanted."
    );
    for row in batch.requests {
        append_spawn_audit(
            &request.cas_dir,
            &request.session,
            Some(row.id),
            None,
            "dequeue",
            "cancelled",
            &detail,
        );
    }
    if let Err(error) = crate::store::open_prompt_queue_store(&request.cas_dir).and_then(|queue| {
        Ok(queue.enqueue_with_summary(
            "director",
            &request.supervisor,
            &detail,
            Some(&request.session),
            Some("Dequeued spawn requests cancelled"),
        )?)
    }) {
        tracing::warn!(%error, %detail, "could not notify supervisor of cancelled spawn poll");
    }
    super::delivery::wake_daemon_after_enqueue(&request.cas_dir);
}

fn poll_spawn_queue(request: &SpawnPollRequest) -> Result<SpawnBatch, String> {
    let queue =
        super::prompt_queue_jobs::job_thread_spawn_queue(&request.cas_dir).map_err(|error| {
            let detail = format!("Could not open the spawn queue store: {error}");
            append_spawn_audit(
                &request.cas_dir,
                &request.session,
                None,
                None,
                "dequeue",
                "failed",
                &detail,
            );
            detail
        })?;
    let requests = queue.poll(&request.session, 10).map_err(|error| {
        let detail = format!("Spawn queue poll failed: {error}");
        append_spawn_audit(
            &request.cas_dir,
            &request.session,
            None,
            None,
            "dequeue",
            "failed",
            &detail,
        );
        super::prompt_queue_jobs::forget_job_thread_stores(&request.cas_dir);
        detail
    })?;
    for row in &requests {
        append_spawn_audit(
            &request.cas_dir,
            &request.session,
            Some(row.id),
            None,
            "dequeue",
            "accepted",
            row.action.as_str(),
        );
    }
    let stalled = if request.scan_stalls {
        queue.peek(50).ok().map(|pending| {
            super::queue_and_events::stalled_spawn_requests(&pending, chrono::Utc::now(),
                chrono::Duration::seconds(super::queue_and_events::SPAWN_QUEUE_STALL_AGE_SECS), &request.reported)
                .into_iter().map(|row| {
                    let target = row.factory_session.as_deref().unwrap_or("(unscoped)");
                    let detail = format!("Spawn request {} ({}) has been queued since {} without being dequeued. It targets factory session '{}' while this daemon is session '{}'. No worker will start for it until a daemon for that session drains it.", row.id, row.action.as_str(), row.created_at.to_rfc3339(), target, request.session);
                    tracing::warn!(request_id = row.id, "cas-2702: spawn request stalled in the queue without being dequeued");
                    append_spawn_audit(&request.cas_dir, &request.session, Some(row.id), None, "dequeue", "stalled", &detail);
                    (row.id, detail)
                }).collect()
        }).unwrap_or_default()
    } else {
        Vec::new()
    };
    Ok(SpawnBatch { requests, stalled })
}

pub(crate) const WAKE_GATE_READ: &str = "WakeGateRead";
const WAKE_MAX_AGE: Duration = Duration::from_millis(250);
const WAKE_READ_BUDGET: Duration = Duration::from_millis(100);

use super::prompt_queue_jobs::QueueSelection;
use super::queue_and_events::{RecipientWakeEvidence, ToolCallEvidence, WorkerWaitState};
use std::collections::HashMap;

#[derive(Debug)]
pub(crate) struct RecipientInputs {
    pub latest: crate::mcp::tools::service::harness_observation::HarnessObservations,
    pub tool_call: ToolCallEvidence,
    pub has_transcript: bool,
    pub wait: WorkerWaitState,
    active_until: Option<chrono::DateTime<chrono::Utc>>,
    first_prompt_until: Option<chrono::DateTime<chrono::Utc>>,
    unknown_observed_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl RecipientInputs {
    pub(crate) fn wake_evidence(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> RecipientWakeEvidence {
        RecipientWakeEvidence {
            recently_active: self.tool_call == ToolCallEvidence::InFlight
                || self.active_until.is_some_and(|until| {
                    now <= until
                        && now
                            >= until
                                - chrono::Duration::from_std(
                                    super::queue_and_events::CLAUDE_ACTIVE_TRANSCRIPT_WINDOW,
                                )
                                .unwrap()
                }),
            awaiting_first_prompt: self.first_prompt_until.is_some_and(|until| {
                now <= until
                    && now
                        >= until
                            - chrono::Duration::from_std(
                                super::queue_and_events::CLAUDE_FIRST_PROMPT_GRACE,
                            )
                            .unwrap()
            }),
        }
    }
    pub(crate) fn mid_turn(&self) -> Option<bool> {
        if !self.has_transcript {
            return None;
        }
        Some(match (&self.latest.wake, &self.latest.completion) {
            (Some(start), Some(end)) => start.at > end.at,
            (Some(_), None) => true,
            _ => false,
        })
    }
    /// cas-e6d7: the transcript positively records a turn end after the last
    /// turn start. Unlike [`Self::mid_turn`], a read tail with no turn record
    /// (a turn longer than the read window) is not evidence of an ended turn.
    pub(crate) fn turn_completed(&self) -> bool {
        self.has_transcript
            && self.latest.completion.as_ref().is_some_and(|end| {
                self.latest.wake.as_ref().is_none_or(|start| end.at > start.at)
            })
    }
}

struct WakeInputs {
    agents: Vec<cas_types::Agent>,
    recipients: HashMap<String, RecipientInputs>,
    captured_at: Instant,
    worker_wait_loaded: bool,
    unknown_fallback: bool,
}

struct WakeReadOutcome {
    inputs: Result<WakeInputs, String>,
    unknown: WakeInputs,
}

fn unknown_wake_inputs(
    recipients: &HashSet<String>,
    captured_at: Instant,
    now: chrono::DateTime<chrono::Utc>,
) -> WakeInputs {
    WakeInputs {
        agents: vec![],
        recipients: recipients
            .iter()
            .map(|name| {
                (
                    name.clone(),
                    RecipientInputs {
                        latest: Default::default(),
                        tool_call: ToolCallEvidence::Unknown,
                        has_transcript: false,
                        wait: WorkerWaitState {
                            harness_waiting_for_input: false,
                            task_blocked: false,
                        },
                        active_until: None,
                        first_prompt_until: None,
                        unknown_observed_at: Some(now),
                    },
                )
            })
            .collect(),
        captured_at,
        worker_wait_loaded: false,
        unknown_fallback: true,
    }
}

/// Owns the raw batch until its off-loop evidence is ready. All batches pass
/// here, including claim-only batches reconstructed from carried RowFacts.
#[derive(Default)]
pub(crate) struct PromptPreparation {
    pending: Option<QueueSelection>,
    in_flight: Option<u64>,
    generation: u64,
    last_submitted: Option<Instant>,
    ready: Option<WakeInputs>,
    current: Option<WakeInputs>,
    unusable_reads: u8,
    fallback: Option<WakeInputs>,
    // Retained across reads and batches: unavailable registration is not a new boot.
    unknown_first_seen: std::cell::RefCell<HashMap<String, chrono::DateTime<chrono::Utc>>>,
    #[cfg(test)]
    pub(crate) read_delay: Duration,
    #[cfg(test)]
    pub(crate) wake_clock: Option<chrono::DateTime<chrono::Utc>>,
}

impl PromptPreparation {
    /// A new process reusing this pane must not inherit its predecessor's
    /// grace, observations, or registration authority. Keep any owned batch
    /// pending, but reject old in-flight observations and read it again.
    pub(crate) fn forget_pane(&mut self, pane: &str) {
        self.unknown_first_seen.get_mut().remove(pane);
        for inputs in [
            self.current.as_mut(),
            self.ready.as_mut(),
            self.fallback.as_mut(),
        ]
        .into_iter()
        .flatten()
        {
            inputs.recipients.remove(pane);
            inputs
                .agents
                .retain(|agent| !agent.name.eq_ignore_ascii_case(pane));
        }
        self.in_flight = None;
        self.last_submitted = None;
        self.unusable_reads = 0;
    }

    pub(crate) fn wake_now(&self) -> chrono::DateTime<chrono::Utc> {
        #[cfg(test)]
        if let Some(now) = self.wake_clock {
            return now;
        }
        chrono::Utc::now()
    }
    fn remember_unknowns(&self, inputs: &WakeInputs) {
        let mut first_seen = self.unknown_first_seen.borrow_mut();
        for (pane, input) in &inputs.recipients {
            if let Some(observed) = input.unknown_observed_at {
                first_seen.entry(pane.clone()).or_insert(observed);
            }
        }
    }
    pub(crate) fn wake_evidence(&self, pane: &str) -> RecipientWakeEvidence {
        let now = self.wake_now();
        let (mut evidence, unknown_observed_at) = self
            .recipient(pane)
            .map(|input| (input.wake_evidence(now), input.unknown_observed_at))
            .unwrap_or((RecipientWakeEvidence::default(), Some(now)));
        if let Some(observed) = unknown_observed_at {
            let mut sightings = self.unknown_first_seen.borrow_mut();
            let first_seen = sightings.get(pane).copied().unwrap_or_else(|| {
                sightings.insert(pane.into(), observed);
                observed
            });
            evidence.awaiting_first_prompt = now >= first_seen
                && now
                    <= first_seen
                        + chrono::Duration::from_std(
                            super::queue_and_events::CLAUDE_FIRST_PROMPT_GRACE,
                        )
                        .unwrap();
        }
        evidence
    }
    pub(crate) fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
    pub(crate) fn allows_delivery(&self, now: Instant) -> bool {
        self.current.as_ref().is_some_and(|inputs| {
            inputs.unknown_fallback
                || now.saturating_duration_since(inputs.captured_at) <= WAKE_MAX_AGE
        })
    }

    fn take_usable(&mut self, now: Instant) -> Option<QueueSelection> {
        if let Some(inputs) = self.ready.take() {
            if now.saturating_duration_since(inputs.captured_at) <= WAKE_MAX_AGE {
                let mut selection = self.pending.take()?;
                selection.registered_agents = inputs.agents.clone();
                self.remember_unknowns(&inputs);
                self.current = Some(inputs);
                self.fallback = None;
                self.unusable_reads = 0;
                return Some(selection);
            }
            if let Some(fallback) = self.fallback.take() {
                self.remember_unknowns(&fallback);
                self.fallback = Some(fallback);
            }
            self.unusable_reads = self.unusable_reads.saturating_add(1);
        }
        if self.unusable_reads < 3 {
            return None;
        }
        let selection = self.pending.take()?;
        // The job prepared Unknown observations as well as its read result.
        // Releasing an unusable batch does no store/transcript work on control
        // and grants no registered sender privileges from stale evidence.
        self.current = Some(self.fallback.take().unwrap_or(WakeInputs {
            agents: vec![],
            recipients: HashMap::new(),
            captured_at: now,
            worker_wait_loaded: false,
            unknown_fallback: true,
        }));
        self.unusable_reads = 0;
        tracing::warn!(
            "wake inputs unavailable after 3 attempts; releasing batch with Unknown evidence"
        );
        Some(selection)
    }
    pub(crate) fn agents(&self) -> &[cas_types::Agent] {
        self.current
            .as_ref()
            .map(|inputs| inputs.agents.as_slice())
            .unwrap_or(&[])
    }
    pub(crate) fn recipient(&self, pane: &str) -> Option<&RecipientInputs> {
        self.current.as_ref()?.recipients.get(pane)
    }
    pub(crate) fn advance(
        &mut self,
        jobs: &mut QueueJobs,
        selection: Option<QueueSelection>,
        cas_dir: PathBuf,
        supervisor: String,
        workers: Vec<String>,
        now: Instant,
    ) -> Option<QueueSelection> {
        jobs.apply_completions();
        self.collect(jobs);
        if self.in_flight.is_some()
            && self.last_submitted.is_some_and(|submitted| {
                now.saturating_duration_since(submitted)
                    >= super::factory_jobs::in_flight_timeout(WAKE_READ_BUDGET)
            })
        {
            self.in_flight = None;
            self.unusable_reads = self.unusable_reads.saturating_add(1);
            tracing::warn!("prompt wake input job timed out; retaining batch for bounded retry");
        }
        if let Some(selection) = selection {
            debug_assert!(self.pending.is_none());
            if selection.rows.is_empty() {
                return Some(selection);
            }
            // Consecutive batches within the read cadence reuse the same
            // bounded snapshot rather than adding a poll interval to delivery.
            if let Some(inputs) = self.current.as_ref() {
                let covered = selection.rows.iter().all(|row| {
                    if row.target == "all_workers" {
                        workers
                            .iter()
                            .all(|name| inputs.recipients.contains_key(name))
                    } else {
                        let name = if row.target == "supervisor" {
                            &supervisor
                        } else {
                            &row.target
                        };
                        inputs.recipients.contains_key(name)
                    }
                });
                let needs_wait = selection.rows.iter().any(|row| match &row.origin {
                    Some(cas_store::QueueOrigin::RegisteredAgent { agent_id }) => {
                        inputs.agents.iter().any(|agent| {
                            agent.id == *agent_id
                                && agent.role == cas_types::AgentRole::Supervisor
                                && agent.name.eq_ignore_ascii_case(&supervisor)
                        })
                    }
                    _ => super::queue_and_events::self_addressed_reminder(row),
                });
                if !inputs.unknown_fallback
                    && now.saturating_duration_since(inputs.captured_at) < POLL_CADENCE
                    && covered
                    && (!needs_wait || inputs.worker_wait_loaded)
                {
                    let mut selection = selection;
                    selection.registered_agents = inputs.agents.clone();
                    return Some(selection);
                }
            }
            self.unusable_reads = 0;
            self.pending = Some(selection);
        }
        if let Some(selection) = self.take_usable(now) {
            return Some(selection);
        }
        if self.pending.is_none()
            || self.in_flight.is_some()
            || self
                .last_submitted
                .is_some_and(|last| now.saturating_duration_since(last) < POLL_CADENCE)
        {
            return None;
        }
        let rows = &self.pending.as_ref().unwrap().rows;
        let mut recipients = HashSet::new();
        for row in rows {
            if row.target == "all_workers" {
                recipients.extend(workers.iter().cloned());
            } else if row.target == "supervisor" {
                recipients.insert(supervisor.clone());
            } else {
                recipients.insert(row.target.clone());
            }
        }
        let sender_ids: HashSet<String> = rows
            .iter()
            .filter_map(|row| match &row.origin {
                Some(cas_store::QueueOrigin::RegisteredAgent { agent_id }) => {
                    Some(agent_id.clone())
                }
                _ => None,
            })
            .collect();
        // cas-901f: a worker's fired self-reminder wakes on the same
        // waiting-for-input evidence as a supervisor decision.
        let self_reminder = rows
            .iter()
            .any(super::queue_and_events::self_addressed_reminder);
        self.generation += 1;
        let generation = self.generation;
        #[cfg(test)]
        let read_delay = self.read_delay;
        let job_supervisor = supervisor.clone();
        let wake_now = self.wake_now();
        let submitted = jobs.submit_external(
            JobSpec {
                kind: WAKE_GATE_READ,
                key: "WakeGateRead:delivery".into(),
                lane: Lane::Blocking,
                criticality: Criticality::Refresh,
                generation,
                budget: WAKE_READ_BUDGET,
            },
            move || {
                let captured_at = Instant::now();
                #[cfg(test)]
                std::thread::sleep(read_delay);
                let inputs = read_wake_inputs(
                    &cas_dir,
                    &job_supervisor,
                    &recipients,
                    &sender_ids,
                    self_reminder,
                    captured_at,
                    wake_now,
                );
                super::delivery::wake_daemon_after_enqueue(&cas_dir);
                Box::new(WakeReadOutcome {
                    inputs,
                    unknown: unknown_wake_inputs(&recipients, captured_at, wake_now),
                })
            },
        );
        if matches!(
            submitted,
            Submitted::Queued
                | Submitted::QueuedBehindKey
                | Submitted::QueuedDroppedOldest { .. }
                | Submitted::Coalesced { .. }
        ) {
            self.in_flight = Some(generation);
        } else {
            self.unusable_reads = self.unusable_reads.saturating_add(1);
        }
        self.last_submitted = Some(now);
        jobs.apply_completions();
        self.collect(jobs);
        self.take_usable(Instant::now())
    }

    fn collect(&mut self, jobs: &mut QueueJobs) {
        for result in jobs.take_external_completions_where(|kind| kind == WAKE_GATE_READ) {
            if self.in_flight != Some(result.generation) {
                continue;
            }
            self.in_flight = None;
            self.unusable_reads = self.unusable_reads.saturating_add(1);
            match result.outcome {
                Ok(ExternalOutcome::Done(value)) => match value.downcast::<WakeReadOutcome>() {
                    Ok(value) => {
                        match value.inputs {
                            Ok(inputs) => {
                                self.unusable_reads = self.unusable_reads.saturating_sub(1);
                                self.ready = Some(inputs);
                            }
                            Err(error) => {
                                self.remember_unknowns(&value.unknown);
                                tracing::warn!(%error, "prompt wake inputs unavailable; retaining batch")
                            }
                        }
                        self.fallback = Some(value.unknown);
                    }
                    Err(_) => tracing::error!("invalid prompt wake input result; retaining batch"),
                },
                _ => tracing::warn!("prompt wake input job did not complete; retaining batch"),
            }
        }
    }
}

fn read_wake_inputs(
    cas_dir: &std::path::Path,
    supervisor: &str,
    recipients: &HashSet<String>,
    sender_ids: &HashSet<String>,
    self_reminder: bool,
    captured_at: Instant,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<WakeInputs, String> {
    let agents = crate::store::open_agent_store(cas_dir)
        .and_then(|store| Ok(store.list(None)?))
        .map_err(|error| error.to_string())?;
    let need_worker_wait = self_reminder || agents.iter().any(|agent| {
        sender_ids.contains(&agent.id)
            && agent.role == cas_types::AgentRole::Supervisor
            && agent.name.eq_ignore_ascii_case(supervisor)
    });
    let blocked: HashSet<String> = if need_worker_wait {
        crate::store::open_task_store_cached(cas_dir)
            .ok()
            .and_then(|tasks| tasks.list(Some(cas_types::TaskStatus::Blocked)).ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|task| task.assignee)
            .map(|name| name.to_ascii_lowercase())
            .collect()
    } else {
        HashSet::new()
    };
    let mut observations = HashMap::new();
    for name in recipients {
        let Some(agent) = crate::cli::factory::wedged::preferred_worker_registration(&agents, name)
        else {
            observations.insert(
                name.clone(),
                RecipientInputs {
                    latest: Default::default(),
                    tool_call: ToolCallEvidence::Unknown,
                    has_transcript: false,
                    active_until: None,
                    first_prompt_until: None,
                    unknown_observed_at: Some(now),
                    wait: WorkerWaitState {
                        harness_waiting_for_input: false,
                        task_blocked: blocked.contains(&name.to_ascii_lowercase()),
                    },
                },
            );
            continue;
        };
        let cli = crate::mcp::tools::service::factory_ops::worker_cli_from_agent(agent);
        let path = crate::mcp::tools::service::factory_ops::worker_transcript_path_for_agent(
            cas_dir, agent,
        )
        .filter(|path| path.exists());
        let latest = path
            .as_ref()
            .map(|path| {
                crate::mcp::tools::service::harness_observation::latest_turn_observations(path, cli)
            })
            .unwrap_or_default();
        let tool_call = path
            .as_ref()
            .map(|path| {
                ToolCallEvidence::from_transcript(
                    crate::cli::factory::wedged::transcript_has_in_flight_tool_call(path, cli),
                )
            })
            .unwrap_or(ToolCallEvidence::Unknown);
        let active_until = path
            .as_ref()
            .and_then(|path| std::fs::metadata(path).ok()?.modified().ok())
            .map(|modified| {
                chrono::DateTime::<chrono::Utc>::from(modified)
                    + chrono::Duration::from_std(
                        super::queue_and_events::CLAUDE_ACTIVE_TRANSCRIPT_WINDOW,
                    )
                    .unwrap()
            });
        let first_prompt_until =
            (path.is_none() && cli == cas_mux::SupervisorCli::Claude).then(|| {
                agent.registered_at
                    + chrono::Duration::from_std(super::queue_and_events::CLAUDE_FIRST_PROMPT_GRACE)
                        .unwrap()
            });
        let harness_waiting_for_input = need_worker_wait
            && name != supervisor
            && crate::ui::factory::director::idle_worker_liveness(cas_dir, name).is_some_and(
                |observation| {
                    observation.state
                        == crate::mcp::tools::service::worker_liveness::Liveness::WaitingForInput
                },
            );
        observations.insert(
            name.clone(),
            RecipientInputs {
                latest,
                tool_call,
                has_transcript: path.is_some(),
                active_until,
                first_prompt_until,
                unknown_observed_at: None,
                wait: WorkerWaitState {
                    harness_waiting_for_input,
                    task_blocked: blocked.contains(&name.to_ascii_lowercase()),
                },
            },
        );
    }
    Ok(WakeInputs {
        agents,
        recipients: observations,
        captured_at,
        worker_wait_loaded: need_worker_wait,
        unknown_fallback: false,
    })
}
