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
            match result.outcome {
                Ok(ExternalOutcome::Done(value)) => self.ready.push_back(
                    value
                        .downcast::<Result<SpawnBatch, String>>()
                        .map(|value| *value)
                        .unwrap_or_else(|_| Err("spawn poll returned an invalid result".into())),
                ),
                other => self
                    .ready
                    .push_back(Err(format!("spawn poll did not finish: {other:?}"))),
            }
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
