//! The read half of the director refresh, separable from the loop (cas-ee9ab).
//!
//! [`FactoryApp::refresh_data`] used to read the store and apply the result in
//! one call on the daemon loop. The read (every task, agent and recent event,
//! plus git changes when due) is the part that costs 100 ms or more on a large
//! store. A [`DirectorRefreshRequest`] captures what the read needs, runs on any
//! thread, and returns a [`DirectorRefreshLoad`] the app applies on the loop.
//! The delivery stage's own fresh read is split the same way
//! ([`DeliveryRequest`] and [`DeliveryInputs`]).
//!
//! Factory loop v2 (cas-223f): the daemon runs these reads as jobs on the
//! jobs runtime's read lane (`DirectorSnapshot`, `RevalidateFresh`). The
//! snapshot also carries the supervisor-stall delivery evidence
//! ([`super::delivery_evidence`]), the session metadata the supervisor-stall
//! check reads and the stall candidates' transcript fingerprints (cas-3b16),
//! the delivery read is joined with the idle-worker liveness observed by a
//! blocking-lane job ([`IdleLivenessRequest`]), and [`DeliverabilityRequest`]
//! is the inject loop's final fresh re-check. Revalidation never decides from
//! cached data; see `revalidate_and_prompt_for_delivery_from` for what the
//! cached snapshot still does on a failed read.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

use cas_factory::SourceChangesInfo;

use super::delivery_evidence::{CiStatusRequest, StallExternalEvidence, ParkedFactsMap, StallEvidenceRequest};
use super::assignment_evidence::{AssignmentEvidence, AssignmentRequest};
use super::{
    CasDbFingerprint, FactoryApp, PhantomWorkerTarget, SessionEpicFocus, SessionProgressRead,
    merge_director_data_preserving_git, preferred_epic_focus_from_session_metadata_named,
    session_progress_named, unfiltered_snapshot_from,
};
use crate::mcp::tools::service::worker_liveness::Observation;
use crate::ui::factory::director::{
    DirectorData, DirectorEvent, DirectorStores, Prompt, TranscriptFingerprint,
    transcript_fingerprint,
};

/// Each idle worker's liveness, `None` when it could not be observed.
pub(crate) type IdleLivenessMap = HashMap<String, Option<Observation>>;

/// The stall candidates' transcript fingerprints, by path (cas-3b16).
pub(crate) type TranscriptFingerprints = HashMap<PathBuf, TranscriptFingerprint>;

/// Everything a director refresh read needs, captured on the loop.
#[derive(Debug, Clone)]
pub(crate) struct DirectorRefreshRequest {
    cas_dir: PathBuf,
    worktree_root: Option<PathBuf>,
    git_due: bool,
    previous_fingerprint: Option<CasDbFingerprint>,
    agent_id_to_name: HashMap<String, String>,
    project_id: Option<String>,
    /// The app's refresh clock when the request was made. A result whose
    /// clock no longer matches is older than a refresh applied since, and is
    /// discarded rather than rolling panel data back.
    requested_after: Instant,
    /// The supervisor-stall evidence to compute with the snapshot
    /// (`None` without a factory session). Assignment ownership is checked
    /// across the whole ready queue even when no epic is focused.
    stall: Option<StallEvidenceRequest>,
    /// The factory session whose metadata the supervisor-stall check reads
    /// (cas-3b16: read here, not on the loop).
    session: Option<String>,
    /// Transcripts of the stall candidates whose cached verdict a write
    /// invalidates (cas-3b16: stat here, not on the loop).
    transcript_paths: Vec<PathBuf>,
    /// The session whose epic focus a full read resolves (`CAS_FACTORY_SESSION`,
    /// captured on the loop; cas-4491f: the metadata is read here).
    focus_session: Option<String>,
    /// The worker panes, whose registry liveness (a `/proc` check per
    /// agent) a full read classifies (cas-4491f).
    worker_targets: Vec<PhantomWorkerTarget>,
}

/// The result of a director refresh read.
pub(crate) struct DirectorRefreshLoad {
    fingerprint: CasDbFingerprint,
    git_due: bool,
    director: DirectorLoad,
    requested_after: Instant,
    /// The store half of the supervisor-stall evidence.
    facts: Option<ParkedFactsMap>,
    pub(super) assignment: Option<AssignmentRequest>,
    /// The session metadata, read with the snapshot.
    session: Option<SessionProgressRead>,
    transcript_fingerprints: TranscriptFingerprints,
    /// Unless the tick was unchanged (cas-4491f): the session's epic focus
    /// and every registered agent, for session mappings and phantom panes.
    /// `None` when unread or unreadable.
    focus: Option<SessionEpicFocus>,
    all_agents: Option<Vec<cas_types::Agent>>,
    /// With a full read: the worker panes whose registry rows are all
    /// non-live (cas-e98e), classified here because liveness checks each
    /// agent's process.
    phantom_workers: Option<Vec<PhantomWorkerTarget>>,
    /// cas-df315: the project identity re-resolved with this read, when it
    /// differs from the scope the request carried (an identity correction
    /// while the factory runs). The loop adopts it as its project scope.
    project_id: Option<String>,
}

impl DirectorRefreshLoad {
    /// The store facts read with this snapshot, for a caller that resolves
    /// the git half on its own thread.
    pub(super) fn facts(&self) -> Option<&ParkedFactsMap> {
        self.facts.as_ref()
    }
}

enum DirectorLoad {
    /// The database is unchanged and git is not due: nothing was read.
    Unchanged,
    Loaded(anyhow::Result<DirectorData>),
    GitOnly(anyhow::Result<Vec<SourceChangesInfo>>),
}

impl DirectorRefreshRequest {
    /// Read the store (and git when due). Runs on any thread.
    pub(crate) fn read(&self, stores: Option<&DirectorStores>) -> DirectorRefreshLoad {
        self.read_with_process_table(stores, &crate::cli::factory::wedged::RealProcessTable)
    }

    pub(crate) fn read_with_process_table(
        &self,
        stores: Option<&DirectorStores>,
        process_table: &dyn crate::cli::factory::wedged::ProcessTable,
    ) -> DirectorRefreshLoad {
        let fingerprint = CasDbFingerprint::from_cas_dir(&self.cas_dir);
        let db_changed = self.previous_fingerprint != Some(fingerprint);
        // cas-df315: the identity was resolved once at startup, so a corrected
        // [project] pin or remote kept the factory scoped to the old project
        // and hid the pinned epic. Re-resolve it here, off the loop, with a
        // database change or on the git cadence; a change forces a full read.
        let corrected = (self.project_id.is_some() && (db_changed || self.git_due))
            .then(|| crate::cloud::resolve_canonical_id(&self.cas_dir))
            .flatten()
            .filter(|resolved| self.project_id.as_deref() != Some(resolved.as_str()));
        let project_id = corrected.as_deref().or(self.project_id.as_deref());
        let director = if db_changed || corrected.is_some() {
            DirectorLoad::Loaded(DirectorData::load_with_stores_for_project(
                &self.cas_dir,
                self.worktree_root.as_deref(),
                self.git_due,
                stores,
                project_id,
            ))
        } else if self.git_due {
            DirectorLoad::GitOnly(DirectorData::load_git_changes_with_stores(
                &self.cas_dir,
                self.worktree_root.as_deref(),
                &self.agent_id_to_name,
                stores,
            ))
        } else {
            DirectorLoad::Unchanged
        };
        let fresh = match &director {
            DirectorLoad::Loaded(Ok(data)) => Some(data),
            _ => None,
        };
        let facts = self.stall.as_ref().map(|stall| stall.facts(fresh, stores));
        // Everything but an unchanged tick applies the focus and the agents.
        let full_read = !matches!(director, DirectorLoad::Unchanged);
        let focus = full_read.then(|| {
            self.focus_session
                .as_deref()
                .map(preferred_epic_focus_from_session_metadata_named)
                .unwrap_or_default()
        });
        let all_agents = if full_read {
            match stores {
                Some(stores) => cas_store::AgentStore::list(&stores.agent_store, None).ok(),
                None => crate::store::open_agent_store(&self.cas_dir)
                    .ok()
                    .and_then(|store| store.list(None).ok()),
            }
        } else {
            None
        };
        let phantom_workers = match (&director, all_agents.as_deref()) {
            (DirectorLoad::Loaded(Ok(_)), Some(agents)) => Some(
                self.worker_targets
                    .iter()
                    .filter(|target| !target.should_keep_with_table(agents, process_table))
                    .cloned()
                    .collect(),
            ),
            _ => None,
        };
        let assignment = self.stall.as_ref().and_then(|stall| stall.assignment_request(fresh));
        DirectorRefreshLoad {
            fingerprint,
            git_due: self.git_due,
            director,
            requested_after: self.requested_after,
            facts,
            assignment,
            session: self.session.as_deref().map(session_progress_named),
            transcript_fingerprints: self
                .transcript_paths
                .iter()
                .map(|path| (path.clone(), transcript_fingerprint(path)))
                .collect(),
            focus,
            all_agents,
            phantom_workers,
            project_id: corrected,
        }
    }
}

/// What the delivery stage's fresh read needs.
#[derive(Debug, Clone)]
pub(crate) struct DeliveryRequest {
    cas_dir: PathBuf,
    worktree_root: Option<PathBuf>,
    project_id: Option<String>,
    /// Workers named by `WorkerIdle` events, whose liveness (agent store,
    /// config, `/proc`) is observed with the read. The daemon moves them to
    /// an [`IdleLivenessRequest`] on the blocking lane
    /// ([`Self::take_idle_liveness`]); a synchronous read observes them here.
    idle_workers: Vec<String>,
    /// Delivery evidence for revalidating a `SupervisorStalled` event.
    stall: Option<StallEvidenceRequest>,
    /// The session whose metadata a `SupervisorStalled` event is revalidated
    /// against (cas-3b16: read here, not on the loop).
    session: Option<String>,
    merge_events: Vec<DirectorEvent>,
    blocked_tasks: Vec<String>,
    supervisor: String,
}

/// The idle-worker liveness a delivery read is joined with: agent store,
/// config, transcripts and `/proc`, so the daemon runs it on the blocking
/// lane (cas-3b16, C2 review round 2, L2).
#[derive(Debug, Clone)]
pub(crate) struct IdleLivenessRequest {
    cas_dir: PathBuf,
    workers: Vec<String>,
}

impl IdleLivenessRequest {
    /// Observe each worker. Runs on any thread.
    pub(crate) fn read(&self) -> IdleLivenessMap {
        self.workers
            .iter()
            .map(|worker| {
                (
                    worker.clone(),
                    crate::ui::factory::director::idle_worker_liveness(&self.cas_dir, worker),
                )
            })
            .collect()
    }
}

/// The delivery stage's fresh read: the unfiltered snapshot events are
/// revalidated against, and the `Blocks` dependencies that gate dispatch.
pub(crate) struct DeliveryInputs {
    pub(super) verification_waits: HashMap<String, String>,
    pub(super) data: anyhow::Result<DirectorData>,
    pub(super) blocks_deps: Vec<cas_types::Dependency>,
    /// Liveness of each idle worker, observed off the loop.
    pub(super) idle_liveness: HashMap<String, Option<Observation>>,
    /// Supervisor-stall store facts against the fresh read; `None` when the
    /// read failed (uncertain) or no `SupervisorStalled` event needs them.
    pub(super) facts: Option<ParkedFactsMap>,
    pub(super) assignment: Option<AssignmentEvidence>,
    /// The session metadata a `SupervisorStalled` event is revalidated
    /// against, read with the facts.
    pub(super) session: Option<SessionProgressRead>,
    pub(super) merge_alerts: HashMap<String, super::merge_alert::Ready>,
    /// Current task/tip/CI and positive build waits, read off the loop.
    pub(super) worker_waits: HashMap<String, super::merge_alert::WorkerWait>,
}

impl DeliveryInputs {
    /// The fresh unfiltered snapshot alone (cas-d2a2: inbox retraction's
    /// `RevalidateFresh` read).
    /// A delivery read whose fresh read failed.
    #[cfg(test)]
    pub(crate) fn failed_for_test() -> Self {
        Self {
            verification_waits: HashMap::new(),
            data: Err(anyhow::anyhow!("test: the fresh read failed")),
            blocks_deps: Vec::new(),
            idle_liveness: HashMap::new(),
            facts: None,
            assignment: None,
            session: None,
            merge_alerts: HashMap::new(),
            worker_waits: HashMap::new(),
        }
    }

    /// The idle-worker liveness joined with this read (tests).
    #[cfg(test)]
    pub(crate) fn idle_liveness_for_test(&self) -> &IdleLivenessMap {
        &self.idle_liveness
    }

    /// Join the idle-worker liveness observed beside this read.
    pub(crate) fn set_idle_liveness(&mut self, liveness: IdleLivenessMap) {
        self.idle_liveness = liveness;
    }

    /// Whether the fresh read succeeded. A failed read is retried before
    /// its events are delivered unrevalidated (cas-223f review round 2, M3).
    pub(crate) fn is_fresh(&self) -> bool {
        self.data.is_ok()
    }

    pub(crate) fn into_data(self) -> anyhow::Result<DirectorData> {
        self.data
    }
}

impl DeliveryRequest {
    /// Merge and supervisor-stall reads may query CI or peer claims, so they
    /// belong on the external lane rather than the shared store read lane.
    pub(crate) fn needs_external_lane(&self) -> bool { !self.merge_events.is_empty() || self.stall.is_some() }
    /// Move the idle-worker liveness out of this read, for a job of its own
    /// on the blocking lane. `None` when no `WorkerIdle` event needs it.
    pub(crate) fn take_idle_liveness(&mut self) -> Option<IdleLivenessRequest> {
        if self.idle_workers.is_empty() {
            return None;
        }
        Some(IdleLivenessRequest {
            cas_dir: self.cas_dir.clone(),
            workers: std::mem::take(&mut self.idle_workers),
        })
    }

    /// Read the delivery snapshot. Runs on any thread.
    pub(crate) fn read(&self, stores: Option<&DirectorStores>) -> DeliveryInputs {
        let data = DirectorData::load_with_stores_for_project(
            &self.cas_dir,
            self.worktree_root.as_deref(),
            false,
            stores,
            self.project_id.as_deref(),
        );
        let blocks_deps = stores
            .and_then(|stores| {
                cas_store::TaskStore::list_dependencies(
                    &stores.task_store,
                    Some(cas_types::DependencyType::Blocks),
                )
                .ok()
            })
            .unwrap_or_default();
        let idle_liveness = IdleLivenessRequest {
            cas_dir: self.cas_dir.clone(),
            workers: self.idle_workers.clone(),
        }
        .read();
        let facts = match (&self.stall, &data) {
            (Some(stall), Ok(fresh)) => Some(stall.facts(Some(fresh), stores)),
            _ => None,
        };
        let session = match (&self.session, &facts) {
            (Some(session), Some(_)) => Some(session_progress_named(session)),
            _ => None,
        };
        let merge_alerts = data.as_ref().ok().map(|fresh| {
            let repo = self.cas_dir.parent().unwrap_or(&self.cas_dir);
            self.merge_events.iter().filter_map(|event| {
                let ready = super::merge_alert::read(event, fresh, repo)?;
                Some((ready.evidence.task_id.clone(), ready))
            }).collect()
        }).unwrap_or_default();
        let worker_waits = data.as_ref().ok().map(|fresh| {
            let repo = self.cas_dir.parent().unwrap_or(&self.cas_dir);
            self.merge_events.iter().filter_map(|event| {
                super::merge_alert::read_worker_wait(event, fresh, &self.cas_dir, repo)
            }).collect()
        }).unwrap_or_default();
        let assignment = self.stall.as_ref().and_then(|stall|
            data.as_ref().ok().and_then(|fresh| stall.assignment_request(Some(fresh))))
            .map(|request| request.read());
        DeliveryInputs {
            verification_waits: self.blocked_tasks.iter().filter_map(|task| {
                crate::mcp::tools::core::task::block_hold::owned_verification_wait(
                    &self.cas_dir, task, &self.supervisor,
                ).map(|dispatch| (task.clone(), dispatch))
            }).collect(),
            data,
            blocks_deps,
            idle_liveness,
            facts,
            assignment,
            session,
            merge_alerts,
            worker_waits,
        }
    }
}

/// Whether a director prompt must be re-checked against a fresh read right
/// before transport (retractable epic or merge alerts, and taskless-worker
/// alerts that drop once the worker is assigned).
pub(crate) fn prompt_needs_fresh_recheck(prompt: &Prompt) -> bool {
    prompt.retract_epic.is_some()
        || prompt.retract_task.is_some()
        || prompt.drop_if_worker_assigned.is_some()
}

/// Whether a director prompt needs anything read before transport: a fresh
/// re-check ([`prompt_needs_fresh_recheck`]), or an assignment wake whose
/// task state is read at the transport boundary (GH #682, cas-2b0b). The
/// daemon reads both in the `RevalidateFresh` job, never on the loop
/// (cas-4491f).
pub(crate) fn prompt_needs_inject_read(prompt: &Prompt) -> bool {
    prompt_needs_fresh_recheck(prompt)
        || (prompt.stall_nudge_task.is_none()
            && crate::prompt_revalidation::assignment_solicited_task_id(&prompt.text).is_some())
}

/// What the transport-boundary read found for an assignment wake
/// (cas-4491f: read in the `RevalidateFresh` job).
#[derive(Debug, Clone, Default)]
pub(crate) enum AssignmentRead {
    /// Not an assignment wake.
    #[default]
    NotAssignment,
    /// The task moved on for this recipient (GH #682, cas-2b0b): suppress.
    Stale(String, cas_types::TaskStatus),
    /// The task, for the durable assignment row.
    Task(Box<cas_types::Task>),
    /// The task could not be read: uncertainty delivers directly.
    Unavailable,
    /// Not read at all: the re-check that would have read it failed or
    /// timed out. The full read, the stale check and the assignment row then
    /// happen on the store worker (cas-4491f review M4, GH #682).
    Unread,
}

/// What [`resolve_unread_assignment`] did with an assignment wake whose task
/// was never read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UnreadAssignmentOutcome {
    /// Not an assignment wake: deliver it as usual.
    NotAssignment,
    /// The task moved on for this recipient: suppressed (GH #682).
    Suppressed,
    /// The durable assignment row carries the wake.
    AssignmentRow,
    /// The assignment policy writes no row (the task is gated, or its
    /// worker is not live), as on the read path.
    Declined,
    /// The task or its row could not be written; the wake went to the
    /// durable prompt queue instead.
    Queued,
    /// Nothing could carry the wake.
    Lost,
}

/// An assignment wake whose re-check never read its task (cas-4491f review
/// M4): read it now, suppress a stale one, and hand a live one to the
/// durable assignment row, as the inject path does after a successful
/// re-check. Blocks on the stores, so the daemon runs it on its store
/// worker.
pub(crate) fn resolve_unread_assignment(
    cas_dir: &std::path::Path,
    factory_session: &str,
    prompt: &Prompt,
) -> UnreadAssignmentOutcome {
    let queue_it = || {
        match crate::ui::factory::daemon::runtime::delivery::enqueue_director_prompt(
            cas_dir,
            factory_session,
            &prompt.target,
            &prompt.text,
        ) {
            Ok(_) => UnreadAssignmentOutcome::Queued,
            Err(error) => {
                tracing::warn!(%error, target_agent = %prompt.target, "assignment wake could not be queued; it may be lost");
                UnreadAssignmentOutcome::Lost
            }
        }
    };
    match AssignmentRead::read(cas_dir, None, prompt) {
        AssignmentRead::NotAssignment => UnreadAssignmentOutcome::NotAssignment,
        AssignmentRead::Stale(task_id, status) => {
            tracing::info!(
                target: "cas::coordination",
                stage = "suppress_terminal_assignment",
                target_agent = %prompt.target,
                task_id = %task_id,
                status = %status,
                "cas-2b0b: suppressed a direct assignment for a terminal task"
            );
            UnreadAssignmentOutcome::Suppressed
        }
        AssignmentRead::Task(task) => match crate::task_assignment::enqueue(cas_dir, &task) {
            Ok(Some(_)) => UnreadAssignmentOutcome::AssignmentRow,
            Ok(None) => UnreadAssignmentOutcome::Declined,
            Err(error) => {
                tracing::warn!(%error, "assignment enqueue failed; queueing the director wake instead");
                queue_it()
            }
        },
        // Unreadable even now: the durable queue delivers it with retries.
        AssignmentRead::Unavailable | AssignmentRead::Unread => queue_it(),
    }
}

impl AssignmentRead {
    /// Read the assignment state `prompt` solicits. Blocks on the task
    /// store; `stores` is the read lane's connection.
    pub(crate) fn read(
        cas_dir: &std::path::Path,
        stores: Option<&DirectorStores>,
        prompt: &Prompt,
    ) -> Self {
        if prompt.stall_nudge_task.is_some() {
            return Self::NotAssignment;
        }
        let Some(task_id) = crate::prompt_revalidation::assignment_solicited_task_id(&prompt.text)
        else {
            return Self::NotAssignment;
        };
        let task = match stores {
            Some(stores) => cas_store::TaskStore::get(&stores.task_store, &task_id).ok(),
            None => crate::store::open_task_store_local(cas_dir)
                .ok()
                .and_then(|store| store.get(&task_id).ok()),
        };
        let Some(task) = task else {
            return Self::Unavailable;
        };
        match crate::prompt_revalidation::assignment_stale_task(
            &prompt.text,
            task.status,
            task.assignee.as_deref(),
            &prompt.target,
        ) {
            Some((task_id, status)) => Self::Stale(task_id, status),
            None => Self::Task(Box::new(task)),
        }
    }
}

/// The inject loop's final re-check (`RevalidateFresh`): one fresh read for a
/// batch of prompts, taken after the tick's epic merges.
#[derive(Debug, Clone)]
pub(crate) struct DeliverabilityRequest {
    cas_dir: PathBuf,
    worktree_root: Option<PathBuf>,
    repo_root: PathBuf,
    /// cas-df315: the app's current scope, not the read lane's startup copy.
    project_id: Option<String>,
}

impl DeliverabilityRequest {
    /// Final merge-signal builder check, executed on the external release lane.
    pub(crate) fn refresh_merge_prompt(&self, prompt: &mut Prompt) -> bool {
        if prompt.merge_alert.is_none() {
            return self.read(None, std::slice::from_mut(prompt)) == [true];
        }
        let Ok(data) = DirectorData::load_with_stores_for_project(
            &self.cas_dir, self.worktree_root.as_deref(), false, None, self.project_id.as_deref(),
        ) else { return false; };
        crate::ui::factory::director::refresh_ci_merge_signal(prompt, &data, &self.cas_dir, &self.repo_root)
    }

    /// Refresh each prompt's body and return whether it is deliverable.
    /// Runs on any thread. Always a
    /// fresh read: a failed read is uncertainty, and an uncertain prompt is
    /// delivered. Never retract a legitimate notification from cached data
    /// (app/mod.rs, `prompt_is_still_deliverable`).
    pub(crate) fn read(&self, stores: Option<&DirectorStores>, prompts: &mut [Prompt]) -> Vec<bool> {
        if !prompts.iter().any(prompt_needs_fresh_recheck) {
            return vec![true; prompts.len()];
        }
        let fresh = DirectorData::load_with_stores_for_project(
            &self.cas_dir,
            self.worktree_root.as_deref(),
            false,
            stores,
            self.project_id.as_deref(),
        );
        prompts
            .iter_mut()
            .map(|prompt| {
                !prompt_needs_fresh_recheck(prompt)
                    || fresh.as_ref().map_or(true, |data| {
                        if prompt.merge_alert.is_some() {
                            return crate::ui::factory::director::refresh_ci_merge_signal(
                                prompt, data, &self.cas_dir, &self.repo_root,
                            );
                        }
                        crate::ui::factory::director::prompt_is_still_deliverable(
                            prompt,
                            data,
                            &self.repo_root,
                        )
                    })
            })
            .collect()
    }

    /// Each prompt's assignment state (cas-4491f). Runs on any thread.
    pub(crate) fn read_assignments(
        &self,
        stores: Option<&DirectorStores>,
        prompts: &[Prompt],
    ) -> Vec<AssignmentRead> {
        prompts
            .iter()
            .map(|prompt| AssignmentRead::read(&self.cas_dir, stores, prompt))
            .collect()
    }
}

/// Run one step of applying a refresh on the loop, and name it when it is
/// slow: a pass budget is 100 ms, and the step that spends it must be
/// findable from the log alone (cas-ee9ab). On the daemon's lane the step
/// is a watchdog step with the 10 ms control budget instead: the slow ring,
/// the status line and the `cas::loop_watchdog` WARN name it, so this log
/// only covers calls off the lane (cas-6931).
pub(crate) fn timed<T>(step: &'static str, run: impl FnOnce() -> T) -> T {
    use crate::ui::factory::daemon::runtime::loop_watchdog;
    let watched = loop_watchdog::scoped(loop_watchdog::Step::control(step));
    if watched.is_some() {
        return run();
    }
    let started = Instant::now();
    let value = run();
    let elapsed = started.elapsed();
    if elapsed >= std::time::Duration::from_millis(50) {
        tracing::warn!(
            target: "cas::refresh_profile",
            step,
            elapsed_ms = elapsed.as_millis() as u64,
            "slow director refresh step on the daemon loop"
        );
    }
    value
}

impl FactoryApp {
    /// The project the director's loads are scoped to (cas-0c98).
    pub(crate) fn project_scope(&self) -> Option<String> {
        self.director_stores
            .as_ref()
            .and_then(|stores| stores.project_id.clone())
    }

    /// Capture what the next director refresh read needs.
    pub(crate) fn director_refresh_request(&self) -> DirectorRefreshRequest {
        DirectorRefreshRequest {
            cas_dir: self.cas_dir.clone(),
            worktree_root: self.worktree_manager.as_ref().map(|m| m.worktree_root()),
            git_due: !self.director_data.git_loaded
                || self.last_git_refresh.elapsed() >= self.git_refresh_interval,
            previous_fingerprint: self.last_db_fingerprint,
            agent_id_to_name: self.director_data.agent_id_to_name.clone(),
            project_id: self.project_scope(),
            requested_after: self.last_refresh,
            stall: self.stall_evidence_request(),
            session: self.factory_session.clone(),
            transcript_paths: self.event_detector.stall_transcript_paths(),
            focus_session: std::env::var("CAS_FACTORY_SESSION").ok(),
            worker_targets: self.worker_names.iter()
                .map(|name| self.phantom_worker_target(name)).collect(),
        }
    }

    /// The supervisor-stall evidence request, when a factory session exists.
    /// Parked deliveries use the focus when present; assignment ownership
    /// spans every ready task so fallback work is covered without focus too.
    fn stall_evidence_request(&self) -> Option<StallEvidenceRequest> {
        self.factory_session.as_ref()?;
        let epic_id = self
            .epic_state
            .epic_id()
            .or(self.current_epic_id.as_deref())
            .map(str::to_string);
        Some(StallEvidenceRequest::new(
            self.cas_dir.clone(),
            epic_id,
            self.factory_session.clone()?,
            &self.unfiltered_director_data,
        ))
    }

    /// External evidence for parked deliveries and ready assignment candidates.
    /// With neither, there is no job (cas-223f review M1, cas-7806).
    pub(crate) fn ci_status_request(&self) -> Option<CiStatusRequest> {
        let assignment = self.stall_evidence_request().and_then(|stall| stall.assignment_request(None));
        CiStatusRequest::for_stall(
            self.delivery_repo_root(), &self.parked_facts,
            std::sync::Arc::clone(&self.lane_ci_pending),
            std::sync::Arc::clone(&self.delivery_containment), assignment,
        )
    }

    /// Apply a finished `CiStatus` job.
    pub(crate) fn apply_ci_status(&mut self, evidence: StallExternalEvidence) {
        self.git_evidence = evidence.git;
        self.assignment_evidence = evidence.assignment.unwrap_or_default();
    }

    /// Capture what the delivery stage's fresh read needs to revalidate
    /// `events`.
    pub(crate) fn delivery_request(&self, events: &[DirectorEvent]) -> DeliveryRequest {
        let idle_workers = events
            .iter()
            .filter_map(|event| match event {
                DirectorEvent::WorkerIdle { worker, .. } => Some(worker.clone()),
                _ => None,
            })
            .collect();
        let stall = events
            .iter()
            .any(|event| matches!(event, DirectorEvent::SupervisorStalled { .. }))
            .then(|| self.stall_evidence_request())
            .flatten();
        let session = stall.as_ref().and(self.factory_session.clone());
        DeliveryRequest {
            blocked_tasks: events.iter().filter_map(|event| match event {
                DirectorEvent::TaskBlocked { task_id, .. } => Some(task_id.clone()),
                _ => None,
            }).collect(),
            supervisor: self.supervisor_name.clone(),
            cas_dir: self.cas_dir.clone(),
            worktree_root: self.worktree_manager.as_ref().map(|m| m.worktree_root()),
            project_id: self.project_scope(),
            idle_workers,
            stall,
            session,
            merge_events: events.iter().filter(|event| super::merge_alert::needs_ci_read(event)).cloned().collect(),
        }
    }

    /// Capture what the inject loop's final fresh re-check needs.
    pub(crate) fn deliverability_request(&self) -> DeliverabilityRequest {
        DeliverabilityRequest {
            cas_dir: self.cas_dir.clone(),
            worktree_root: self.worktree_manager.as_ref().map(|m| m.worktree_root()),
            repo_root: self.delivery_repo_root(),
            project_id: self.project_scope(),
        }
    }

    /// Apply a finished director read: merge it into panel data and detect
    /// state changes. In-memory work plus the session metadata files.
    pub(crate) fn apply_director_refresh(
        &mut self,
        load: DirectorRefreshLoad,
    ) -> anyhow::Result<Vec<DirectorEvent>> {
        if load.requested_after != self.last_refresh {
            // A newer refresh landed while this read ran (a synchronous
            // refresh on an operator action, say). Applying the older read
            // would roll panel data and change detection back.
            tracing::debug!("discarding a director refresh read older than the applied one");
            return Ok(Vec::new());
        }
        let git_due = load.git_due;
        if let Some(project_id) = load.project_id
            && let Some(stores) = self.director_stores.as_mut()
        {
            tracing::info!(%project_id, previous = ?stores.project_id, "cas-df315: factory project scope follows the corrected identity");
            stores.project_id = Some(project_id);
        }
        if let Some(facts) = load.facts {
            self.parked_facts = facts;
        }
        if let Some(session) = load.session {
            self.session_progress = session;
        }
        self.event_detector
            .observe_transcript_fingerprints(load.transcript_fingerprints);
        let db_changed = match load.director {
            DirectorLoad::Loaded(loaded) => {
                self.director_data =
                    merge_director_data_preserving_git(&self.director_data, loaded?, git_due);
                // cas-dbbe: snapshot the fresh, still-unfiltered load BEFORE
                // `filter_director_agents_to_current_session()` mutates
                // `self.director_data` in place. Change detection and the
                // TaskCompleted safety net read this canonical copy.
                self.unfiltered_director_data = unfiltered_snapshot_from(&self.director_data);
                // cas-ae6d: the only place `director_data` is genuinely re-read.
                self.director_data_loaded_at = chrono::Utc::now();
                if git_due {
                    self.last_git_refresh = Instant::now();
                }
                true
            }
            DirectorLoad::GitOnly(changes) => {
                self.director_data.changes = changes?;
                self.director_data.git_loaded = true;
                self.last_git_refresh = Instant::now();
                false
            }
            DirectorLoad::Unchanged => {
                timed("branch visibility", || {
                    self.refresh_branch_visibility_cache()
                });
                self.last_refresh = Instant::now();
                // Worker holds live in session metadata rather than cas.db.
                // They must be reconciled even on the unchanged-DB path;
                // otherwise a just-written hold leaks one more WorkerIdle
                // event before unrelated database activity occurs.
                // cas-3b16: read with the snapshot, off the loop.
                timed("worker holds", || self.apply_snapshot_worker_holds());
                let mut events: Vec<_> = timed("supervisor stall", || self.detect_supervisor_stall())
                    .into_iter().collect();
                events.extend(self.merge_alert_retry_events_at(self.merge_alert_retries.clock()));
                return Ok(events);
            }
        };

        timed("branch visibility", || {
            self.refresh_branch_visibility_cache()
        });
        self.last_db_fingerprint = Some(load.fingerprint);
        self.last_refresh = Instant::now();

        // cas-4491f: the agents and the epic focus arrived with the snapshot
        // job; nothing here reads a store or a file.
        // Sync session_id → pane_name mappings from the registered agents.
        if let Some(agents) = load.all_agents.as_deref() {
            timed("session mappings", || self.sync_session_mappings_from(agents));
        }
        if let Some(focus) = load.focus {
            timed("epic focus", || self.apply_epic_focus(focus));
        }
        timed("worker holds", || self.apply_snapshot_worker_holds());

        // cas-e98e AC3: drop phantom worker panes when registry says the
        // worker is no longer supervision-live. Their teardown requests are
        // queued off the loop.
        if db_changed && let Some(phantoms) = load.phantom_workers {
            timed("phantom panes", || self.reconcile_phantom_worker_panes_from(phantoms));
        }

        // Detect state changes against the UNFILTERED snapshot (cas-dbbe), and
        // gate `EpicStarted` on the tracked epic (cas-4181).
        let mut events = timed("change detection", || {
            self.event_detector
                .detect_changes(&self.unfiltered_director_data, self.epic_state.epic_id())
        });
        events.extend(timed("supervisor stall", || self.detect_supervisor_stall()));
        let retries = self.merge_alert_retry_events_at(self.merge_alert_retries.clock());
        for retry in retries {
            if !events.iter().any(|event| match (event, &retry) {
                (DirectorEvent::WorkerIdle { active_task: Some(a), .. },
                 DirectorEvent::WorkerIdle { active_task: Some(b), .. }) => a.task_id == b.task_id,
                _ => false,
            }) {
                events.push(retry);
            }
        }

        // Now filter to current session (agents + tasks scoped to active epic)
        if db_changed {
            self.filter_director_agents_to_current_session();
        }

        Ok(events)
    }
}
