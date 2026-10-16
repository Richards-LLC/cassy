//! C2d: owned spawn ingress and prompt evidence preparation off control.
use std::collections::{HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::jobs::{Criticality, JobSpec, Lane, Submitted};
use super::prompt_queue_jobs::{ExternalOutcome, QueueJobs};
use super::queue_and_events::append_spawn_audit;

pub(crate) const SPAWN_QUEUE_POLL: &str = "SpawnQueuePoll";
pub(crate) const POLL_CADENCE: Duration = Duration::from_millis(100);

#[derive(Debug)]
pub(crate) struct SpawnPollRequest {
    pub cas_dir: PathBuf,
    pub session: String,
    pub scan_stalls: bool,
    pub reported: HashSet<i64>,
}

#[derive(Debug)]
pub(crate) struct SpawnBatch {
    pub requests: Vec<cas_store::SpawnRequest>,
    pub stalled: Vec<(i64, String)>,
}

/// Poll is a dequeue mutation, not a replaceable read. Keep one outstanding
/// command and consume every successful result, even if it arrives late.
#[derive(Default)]
pub(crate) struct SpawnIngress {
    generation: u64,
    in_flight: Option<u64>,
    last_submitted: Option<Instant>,
    ready: VecDeque<Result<SpawnBatch, String>>,
}

impl SpawnIngress {
    pub(crate) fn force_poll(&mut self) {
        self.last_submitted = None;
    }

    pub(crate) fn advance(
        &mut self,
        jobs: &mut QueueJobs,
        request: SpawnPollRequest,
        now: Instant,
    ) -> bool {
        jobs.apply_completions();
        for result in jobs.take_external_completions_where(|kind| kind == SPAWN_QUEUE_POLL) {
            // No timeout/resubmission for a mutating poll: it may already
            // have marked durable rows processed. Reset never invalidates it.
            self.in_flight = None;
            let outcome = match result.outcome {
                Ok(ExternalOutcome::Done(value)) => value
                    .downcast::<Result<SpawnBatch, String>>()
                    .map(|value| *value)
                    .unwrap_or_else(|_| Err("spawn poll returned an invalid result".into())),
                Ok(ExternalOutcome::Dropped) => Err("critical spawn poll was dropped".into()),
                Err(error) => Err(format!("spawn poll panicked: {error:?}")),
            };
            self.ready.push_back(outcome);
        }
        if self.in_flight.is_some()
            || !self.ready.is_empty()
            || self
                .last_submitted
                .is_some_and(|last| now.saturating_duration_since(last) < POLL_CADENCE)
        {
            return false;
        }
        self.generation += 1;
        let generation = self.generation;
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
                let result = poll_spawn_queue(&request);
                if result
                    .as_ref()
                    .is_ok_and(|batch| !batch.requests.is_empty() || !batch.stalled.is_empty())
                {
                    super::delivery::wake_daemon_after_enqueue(&request.cas_dir);
                }
                Box::new(result)
            },
        );
        let accepted = matches!(
            submitted,
            Submitted::Queued
                | Submitted::QueuedBehindKey
                | Submitted::QueuedDroppedOldest { .. }
                | Submitted::Coalesced { .. }
        );
        if accepted {
            self.in_flight = Some(generation);
            self.last_submitted = Some(now);
        } else {
            self.ready
                .push_back(Err(format!("spawn poll submission refused: {submitted:?}")));
            self.last_submitted = Some(now);
        }
        // Inline fixtures also cross this exact completion path.
        jobs.apply_completions();
        for result in jobs.take_external_completions_where(|kind| kind == SPAWN_QUEUE_POLL) {
            self.in_flight = None;
            let outcome = match result.outcome {
                Ok(ExternalOutcome::Done(value)) => value
                    .downcast::<Result<SpawnBatch, String>>()
                    .map(|value| *value)
                    .unwrap_or_else(|_| Err("spawn poll returned an invalid result".into())),
                Ok(ExternalOutcome::Dropped) => Err("critical spawn poll was dropped".into()),
                Err(error) => Err(format!("spawn poll panicked: {error:?}")),
            };
            self.ready.push_back(outcome);
        }
        accepted
    }

    pub(crate) fn take_ready(&mut self) -> Option<Result<SpawnBatch, String>> {
        self.ready.pop_front()
    }
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
}

impl RecipientInputs {
    pub(crate) fn wake_evidence(
        &self,
        now: chrono::DateTime<chrono::Utc>,
    ) -> RecipientWakeEvidence {
        RecipientWakeEvidence {
            recently_active: self.tool_call == ToolCallEvidence::InFlight
                || self.active_until.is_some_and(|until| now <= until),
            awaiting_first_prompt: self.first_prompt_until.is_some_and(|until| now <= until),
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
}

struct WakeInputs {
    agents: Vec<cas_types::Agent>,
    recipients: HashMap<String, RecipientInputs>,
    finished_at: Instant,
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
}

impl PromptPreparation {
    pub(crate) fn has_pending(&self) -> bool {
        self.pending.is_some()
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
        if let Some(selection) = selection {
            debug_assert!(self.pending.is_none());
            if selection.rows.is_empty() {
                return Some(selection);
            }
            self.pending = Some(selection);
        }
        if let Some(inputs) = self.ready.take() {
            if now.saturating_duration_since(inputs.finished_at) <= WAKE_MAX_AGE {
                let mut selection = self.pending.take()?;
                selection.registered_agents = inputs.agents.clone();
                self.current = Some(inputs);
                return Some(selection);
            }
            // A completion held past its usable age is replaceable evidence.
            // The raw selection and its claims remain owned while it refreshes.
            self.last_submitted = None;
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
        self.generation += 1;
        let generation = self.generation;
        let submitted = jobs.submit_external(
            JobSpec {
                kind: WAKE_GATE_READ,
                key: "WakeGateRead:delivery".into(),
                lane: Lane::Blocking,
                criticality: Criticality::Refresh,
                generation,
                budget: Duration::from_millis(100),
            },
            move || {
                let inputs = read_wake_inputs(&cas_dir, &supervisor, &recipients, &sender_ids);
                super::delivery::wake_daemon_after_enqueue(&cas_dir);
                Box::new(inputs)
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
        }
        self.last_submitted = Some(now);
        jobs.apply_completions();
        self.collect(jobs);
        if let Some(inputs) = self.ready.take() {
            let mut selection = self.pending.take()?;
            selection.registered_agents = inputs.agents.clone();
            self.current = Some(inputs);
            return Some(selection);
        }
        None
    }
    fn collect(&mut self, jobs: &mut QueueJobs) {
        for result in jobs.take_external_completions_where(|kind| kind == WAKE_GATE_READ) {
            if self.in_flight != Some(result.generation) {
                continue;
            }
            self.in_flight = None;
            match result.outcome {
                Ok(ExternalOutcome::Done(value)) => match value
                    .downcast::<Result<WakeInputs, String>>()
                {
                    Ok(value) => match *value {
                        Ok(inputs) => self.ready = Some(inputs),
                        Err(error) => {
                            tracing::warn!(%error, "prompt wake inputs unavailable; retaining batch")
                        }
                    },
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
) -> Result<WakeInputs, String> {
    let agents = crate::store::open_agent_store(cas_dir)
        .and_then(|store| Ok(store.list(None)?))
        .map_err(|error| error.to_string())?;
    let need_worker_wait = agents.iter().any(|agent| {
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
        let Some(agent) = agents.iter().find(|agent| agent.name == *name) else {
            observations.insert(
                name.clone(),
                RecipientInputs {
                    latest: Default::default(),
                    tool_call: ToolCallEvidence::Unknown,
                    has_transcript: false,
                    active_until: None,
                    first_prompt_until: None,
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
        finished_at: Instant::now(),
    })
}
