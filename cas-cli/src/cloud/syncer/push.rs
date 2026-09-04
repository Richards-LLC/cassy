use chrono::Utc;
use flate2::Compression;
use flate2::write::GzEncoder;
use std::collections::HashMap;
use std::io::Write;
use std::time::Instant;
use tracing::warn;

use crate::cloud::sync_queue::PendingByType;
use crate::cloud::syncer::{
    CloudSyncer, PushBacklog, PushItemizedFailure, PushPlan, PushResponse, PushRowOutcome,
    PushRowResult, PushScope, SyncResult,
};
use crate::cloud::{QueuedSync, SyncOperation};
use crate::error::CasError;
use crate::types::Session;

/// What one entity-typed batch actually settled.
///
/// The pushed count alone cannot distinguish a row the cloud wrote from a row
/// it declined because it already held a newer version. Both leave the queue,
/// but only the second must be reported as a kept-newer acknowledgement.
#[derive(Debug, Default, Clone)]
pub(super) struct PushBatchOutcome {
    /// Rows removed from the queue (written, or acknowledged as LWW losses).
    pub synced: usize,
    /// Rows the cloud kept a newer version of.
    pub skipped_lww: usize,
    /// A refusal the server explained per row. This travels with the counts
    /// rather than as `Err` so a partially rejected batch still reports the
    /// rows it did settle instead of erasing them behind the first error.
    pub error: Option<String>,
}

impl CloudSyncer {
    pub fn push(&self) -> Result<SyncResult, CasError> {
        self.push_scoped(PushScope::All)
    }

    /// `Some(message)` when this checkout is a scratch/probe root that must not
    /// push (GH #701). Resolution failure yields `None`: an unclassifiable root
    /// syncs, because blocking a real project is the expensive mistake.
    fn ephemeral_project_refusal(&self) -> Option<String> {
        // Classify the root this syncer was built for, not whatever project the
        // process happens to be running in: on CI the process root is the runner
        // workspace, which is not the project being pushed.
        let cas_root = self
            .push_cas_root
            .clone()
            .or_else(|| crate::store::find_cas_root().ok())?;
        let verdict = crate::cloud::classify_project_root(&cas_root);
        let project_id = self
            .push_project_canonical_id
            .clone()
            .or_else(|| crate::cloud::resolve_canonical_id(&cas_root))
            .unwrap_or_else(|| cas_root.display().to_string());
        verdict.explain(&project_id)
    }

    /// Describe the exact next queue batch without mutating it.
    pub fn plan_push(&self, scope: PushScope) -> Result<PushPlan, CasError> {
        let batch_limit = self.config.batch_size.max(1);
        let items = self.queue.pending_for_entity_type(
            scope.entity_type(),
            batch_limit,
            self.config.max_retries,
        )?;
        let total_matching = self
            .queue
            .pending_count_for_entity_type(scope.entity_type(), self.config.max_retries)?;
        let mut counts = scope
            .planned_keys()
            .iter()
            .map(|key| ((*key).to_string(), 0usize))
            .collect::<std::collections::BTreeMap<_, _>>();
        for item in &items {
            *counts
                .entry(item.entity_type.collection_key().to_string())
                .or_default() += 1;
        }

        Ok(PushPlan {
            source: "sync_queue",
            scope,
            counts,
            total_in_next_batch: items.len(),
            total_matching,
            batch_limit,
            batch_limit_reached: items.len() == batch_limit,
        })
    }

    /// Push only queue rows selected by `scope`.
    pub fn push_scoped(&self, scope: PushScope) -> Result<SyncResult, CasError> {
        self.push_scoped_with_sessions(scope, &[])
    }

    /// Push at most `max_batches` queue batches. This is an escape hatch for
    /// operators who need to bound a large backlog; the default remains an
    /// unbounded drain with the per-request limits in [`CloudSyncerConfig`].
    pub fn push_scoped_with_max_batches(
        &self,
        scope: PushScope,
        max_batches: usize,
    ) -> Result<SyncResult, CasError> {
        self.push_scoped_with_sessions_and_limit(scope, &[], Some(max_batches.max(1)))
    }

    /// Push queued changes and sessions to cloud
    pub fn push_with_sessions(&self, sessions: &[Session]) -> Result<SyncResult, CasError> {
        self.push_scoped_with_sessions(PushScope::All, sessions)
    }

    fn push_scoped_with_sessions(
        &self,
        scope: PushScope,
        sessions: &[Session],
    ) -> Result<SyncResult, CasError> {
        self.push_scoped_with_sessions_and_limit(scope, sessions, None)
    }

    fn push_scoped_with_sessions_and_limit(
        &self,
        scope: PushScope,
        sessions: &[Session],
        max_batches: Option<usize>,
    ) -> Result<SyncResult, CasError> {
        let mut result = SyncResult::default();
        let start = Instant::now();

        self.requeue_version_gated_items()?;
        result.requeued_after_upgrade = self.requeue_stale_client_failures()?;

        if !self.is_available() {
            return Ok(result);
        }

        // GH #701: a throwaway checkout must not mint a cloud identity and
        // push into the account's shared buckets. Declining is a no-op, not a
        // failure — the queue is left intact so a later `cas cloud project
        // set` drains it.
        if let Some(refusal) = self.ephemeral_project_refusal() {
            warn!("[Cassy sync] {refusal}");
            return Ok(result);
        }

        let batch_limit = self.config.batch_size.max(1);
        let token = self
            .cloud_config
            .token
            .as_ref()
            .ok_or_else(|| CasError::Other("Not logged in".to_string()))?;

        loop {
            if max_batches.is_some_and(|limit| result.batches_run >= limit) {
                break;
            }

            let pending = match scope.entity_type() {
                Some(entity_type) => self.queue.pending_by_type_for_entity(
                    entity_type,
                    batch_limit,
                    self.config.max_retries,
                )?,
                None => self
                    .queue
                    .pending_by_type(batch_limit, self.config.max_retries)?,
            };

            if pending.is_empty() {
                if !sessions.is_empty() {
                    result.batches_run += 1;
                    match self.push_sessions(sessions, token) {
                        Ok(count) => result.pushed_sessions += count,
                        Err(e) => result.errors.push(format!("Session push failed: {e}")),
                    }
                }
                break;
            }

            let before = self
                .queue
                .pending_count_for_entity_type(scope.entity_type(), self.config.max_retries)?;
            let round = self.push_pending_batch(&pending, token);
            let round_had_errors = !round.errors.is_empty();
            result.batches_run += 1;
            Self::merge_push_result(&mut result, round);

            let after = self
                .queue
                .pending_count_for_entity_type(scope.entity_type(), self.config.max_retries)?;
            if after >= before {
                result.errors.push(format!(
                    "Push stopped after making no progress; {after} matching row(s) remain pending"
                ));
                break;
            }
            if round_had_errors {
                break;
            }
        }

        // Update last push timestamp
        let _ = self
            .queue
            .set_metadata("last_push_at", &Utc::now().to_rfc3339());

        result.remaining_backlog = self.remaining_backlog(scope)?;
        result.duration_ms = start.elapsed().as_millis() as u64;
        Ok(result)
    }

    fn push_pending_batch(&self, pending: &PendingByType, token: &str) -> SyncResult {
        let mut result = SyncResult::default();

        macro_rules! push_type {
            ($field:ident, $items:expr, $label:literal, $error:literal) => {
                if !$items.is_empty() {
                    match self.push_batch($items, $label, token) {
                        Ok(outcome) => {
                            result.$field = outcome.synced;
                            result.skipped_lww_acked += outcome.skipped_lww;
                            if let Some(error) = outcome.error {
                                result.errors.push(format!(concat!($error, ": {}"), error));
                            }
                        }
                        Err(e) => result.errors.push(format!(concat!($error, ": {}"), e)),
                    }
                }
            };
        }

        push_type!(
            pushed_entries,
            &pending.entries,
            "entries",
            "Entry push failed"
        );
        push_type!(pushed_tasks, &pending.tasks, "tasks", "Task push failed");
        push_type!(pushed_rules, &pending.rules, "rules", "Rule push failed");
        push_type!(
            pushed_skills,
            &pending.skills,
            "skills",
            "Skill push failed"
        );
        push_type!(
            pushed_sessions,
            &pending.sessions,
            "sessions",
            "Session push failed"
        );
        push_type!(
            pushed_verifications,
            &pending.verifications,
            "verifications",
            "Verification push failed"
        );
        push_type!(
            pushed_events,
            &pending.events,
            "events",
            "Event push failed"
        );
        push_type!(
            pushed_prompts,
            &pending.prompts,
            "prompts",
            "Prompt push failed"
        );
        push_type!(
            pushed_file_changes,
            &pending.file_changes,
            "file_changes",
            "FileChange push failed"
        );
        push_type!(
            pushed_commit_links,
            &pending.commit_links,
            "commit_links",
            "CommitLink push failed"
        );
        push_type!(
            pushed_agents,
            &pending.agents,
            "agents",
            "Agent push failed"
        );
        push_type!(
            pushed_worktrees,
            &pending.worktrees,
            "worktrees",
            "Worktree push failed"
        );
        push_type!(
            pushed_task_dependencies,
            &pending.task_dependencies,
            "task_dependencies",
            "Task dependency push failed"
        );

        result
    }

    fn merge_push_result(target: &mut SyncResult, source: SyncResult) {
        target.pushed_entries += source.pushed_entries;
        target.pushed_tasks += source.pushed_tasks;
        target.pushed_rules += source.pushed_rules;
        target.pushed_skills += source.pushed_skills;
        target.pushed_sessions += source.pushed_sessions;
        target.pushed_verifications += source.pushed_verifications;
        target.pushed_events += source.pushed_events;
        target.pushed_prompts += source.pushed_prompts;
        target.pushed_file_changes += source.pushed_file_changes;
        target.pushed_commit_links += source.pushed_commit_links;
        target.pushed_agents += source.pushed_agents;
        target.pushed_worktrees += source.pushed_worktrees;
        target.pushed_task_dependencies += source.pushed_task_dependencies;
        target.skipped_lww_acked += source.skipped_lww_acked;
        target.conflicts_resolved += source.conflicts_resolved;
        target.conflicts_resolved_local += source.conflicts_resolved_local;
        target.conflicts_resolved_remote += source.conflicts_resolved_remote;
        target.conflicts.extend(source.conflicts);
        target.errors.extend(source.errors);
    }

    fn remaining_backlog(&self, scope: PushScope) -> Result<PushBacklog, CasError> {
        const ERROR_LIMIT: usize = 20;
        let failed = self
            .queue
            .failed_count_for_entity_type(scope.entity_type(), self.config.max_retries)?;
        let failed_errors = self
            .queue
            .failed_for_entity_type(scope.entity_type(), self.config.max_retries, ERROR_LIMIT)?
            .into_iter()
            .filter_map(|item| {
                item.last_error
                    .map(|error| format!("{} {}: {error}", item.entity_type, item.entity_id))
            })
            .collect();

        Ok(PushBacklog {
            pending: self
                .queue
                .pending_count_for_entity_type(scope.entity_type(), self.config.max_retries)?,
            failed,
            failed_errors,
            rejected_by_reason: self.queue.rejected_reason_counts_for_entity_type(
                scope.entity_type(),
                self.config.max_retries,
            )?,
        })
    }

    /// Push sessions to cloud
    fn push_sessions(&self, sessions: &[Session], token: &str) -> Result<usize, CasError> {
        if sessions.is_empty() {
            return Ok(0);
        }

        let push_url = format!("{}/api/sync/push", self.cloud_config.endpoint);

        let values = serde_json::to_value(sessions)
            .map_err(|e| CasError::Other(format!("JSON serialization failed: {e}")))?
            .as_array()
            .cloned()
            .unwrap_or_default();
        let payload = self.build_personal_push_payload("sessions", values)?;

        let json_bytes = serde_json::to_vec(&payload)
            .map_err(|e| CasError::Other(format!("JSON serialization failed: {e}")))?;
        let compressed = Self::gzip_json(&json_bytes)?;
        self.check_personal_payload_size(json_bytes.len(), compressed.len())?;

        let response = ureq::post(&push_url)
            .timeout(self.config.timeout)
            .set("Authorization", &format!("Bearer {token}"))
            .set("Content-Type", "application/json")
            .set("Content-Encoding", "gzip")
            .send_bytes(&compressed);

        match response {
            Ok(resp) if resp.status() == 200 || resp.status() == 201 => {
                // Update last session push timestamp
                let _ = self
                    .queue
                    .set_metadata("last_session_push_at", &Utc::now().to_rfc3339());
                Ok(sessions.len())
            }
            Ok(resp) => {
                let status = resp.status();
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!(
                    "Session push failed with status {status}: {body}"
                )))
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                Err(CasError::Other(format!(
                    "Session push failed with status {code}: {body}"
                )))
            }
            Err(ureq::Error::Transport(e)) => Err(CasError::Other(format!("Network error: {e}"))),
        }
    }

    /// Declare the base revision this upsert is built on.
    ///
    /// Sent as a decimal string, the wire form the server parses. When this
    /// client has never observed a revision for the row the key is OMITTED
    /// entirely — that is what selects the server's timestamp-LWW compatibility
    /// path. A placeholder would be actively harmful: the server drops a row
    /// whose revision it cannot parse, and a fabricated "0" against an existing
    /// row is a guaranteed conflict.
    fn with_base_revision(
        &self,
        item: &QueuedSync,
        mut payload: serde_json::Value,
    ) -> serde_json::Value {
        let Some(object) = payload.as_object_mut() else {
            return payload;
        };
        // Never let a stale body-embedded revision reach the server; the base
        // is ours to declare, from the ledger only.
        object.remove("revision");
        if let Ok(Some(revision)) = self.queue.revision(item.entity_type, &item.entity_id) {
            object.insert(
                "revision".to_string(),
                serde_json::Value::String(revision.to_string()),
            );
        }
        payload
    }

    /// Store the revisions the server echoed for accepted rows, and drop a base
    /// this push proved stale.
    fn settle_revision_receipts(&self, entity_type: &str, response: &super::PushResponse) {
        let Some(entity) = crate::cloud::EntityType::from_collection_key(entity_type) else {
            return;
        };
        for (id, revision) in response.accepted_revisions_for(entity_type) {
            let _ = self.queue.record_revision(entity, &id, revision);
        }
        for (id, current_revision) in response.revision_conflicts_for(entity_type) {
            // Our base lost the race. Forget it rather than replacing it with
            // the server's current revision: pretending we have seen that row
            // would let the next push overwrite a change we never looked at.
            // The next pull records the real revision and resolves the row.
            let _ = self.queue.clear_revision(entity, &id);
            tracing::debug!(
                entity_type = entity_type,
                entity_id = %id,
                server_revision = ?current_revision,
                "cloud rejected a stale base revision; dropped the local base and left the row queued"
            );
        }
    }

    fn settle_personal_row_results(
        &self,
        batch_items: &[&QueuedSync],
        entity_type: &str,
        raw_response: &str,
        rows: HashMap<String, PushRowResult>,
        outcome: &mut PushBatchOutcome,
        skip_errors: &mut Vec<String>,
    ) {
        let mut rejected = Vec::new();
        for item in batch_items {
            let row = rows
                .get(&item.entity_id)
                .expect("row_results_for validates every queue identity");
            if row.acknowledges() {
                if row.outcome == PushRowOutcome::SkippedLww {
                    outcome.skipped_lww += 1;
                }
                let _ = self.queue.mark_synced(item.id);
                outcome.synced += 1;
                continue;
            }

            let reason = row.reason.as_deref().unwrap_or("unspecified");
            let diagnostic = format!(
                "cloud rejected {entity_type} {}: reason={reason} ({}); server response: {raw_response}",
                item.entity_id,
                crate::cloud::syncer::push_reason_hint(reason)
            );
            let _ = self
                .queue
                .record_row_outcome(item.id, "rejected", Some(reason));
            if row.rejection_is_retryable() {
                let _ = self.queue.mark_failed(item.id, &diagnostic);
            } else {
                let _ = self
                    .queue
                    .park_failed(item.id, &diagnostic, self.config.max_retries);
            }
            rejected.push(format!("{} ({reason})", item.entity_id));
        }

        if !rejected.is_empty() {
            skip_errors.push(format!(
                "cloud rejected {} of {} {entity_type} row(s): {}",
                rejected.len(),
                batch_items.len(),
                rejected.join(", ")
            ));
        }
    }

    fn push_batch(
        &self,
        items: &[QueuedSync],
        entity_type: &str,
        token: &str,
    ) -> Result<PushBatchOutcome, CasError> {
        // Separate upserts and deletes
        let upsert_items: Vec<&QueuedSync> = items
            .iter()
            .filter(|i| i.operation == SyncOperation::Upsert)
            .collect();

        let deletes: Vec<&QueuedSync> = items
            .iter()
            .filter(|i| i.operation == SyncOperation::Delete)
            .collect();

        // Parse payloads to (item, json_value) tuples.
        // Items with a missing or unparseable payload cannot be pushed; park
        // them as failed immediately (mark_failed increments retry_count) so
        // they surface under `failed` in queue stats and don't consume a batch
        // slot on every push cycle.  After max_retries calls they leave
        // `pending` entirely, advancing `oldest_item` past them (defect B /
        // cas-8dd8 poison-head fix).
        let mut upsert_entries: Vec<(&QueuedSync, serde_json::Value)> = Vec::new();
        for item in &upsert_items {
            match item.payload.as_deref() {
                Some(payload) => match serde_json::from_str::<serde_json::Value>(payload) {
                    Ok(v) => upsert_entries.push((*item, self.with_base_revision(item, v))),
                    Err(_) => {
                        let _ = self
                            .queue
                            .mark_failed(item.id, "invalid JSON payload for upsert");
                    }
                },
                None => {
                    let _ = self
                        .queue
                        .mark_failed(item.id, "missing payload for upsert operation");
                }
            }
        }

        let mut outcome = PushBatchOutcome::default();
        // A 2xx response that explicitly reports skipped rows is not a fully
        // successful push. Aggregate-only responses leave every row
        // indistinguishable, while itemized rejected/invalid rows identify
        // exactly which queue records must remain visible for retry.
        let mut skip_errors = Vec::new();

        // Split upserts into size-limited sub-batches (consuming values to avoid cloning)
        if !upsert_entries.is_empty() {
            let sub_batches = self.split_personal_sub_batches(upsert_entries, entity_type)?;

            for sub_batch in sub_batches {
                let (batch_items, values): (Vec<&QueuedSync>, Vec<serde_json::Value>) =
                    sub_batch.into_iter().unzip();

                match self.push_sub_batch(values, entity_type, token) {
                    Ok(response) => {
                        // Independent of how rows settle: a revision receipt is
                        // the server telling us what it now holds.
                        self.settle_revision_receipts(entity_type, &response);
                        // Defensive cross-check against the server-side
                        // `ON CONFLICT DO UPDATE ... WHERE false` silent-skip
                        // path (cas-0bdc / cas-d656): an aggregate skipped
                        // count alone cannot identify which `batch_items`
                        // were dropped, so it conservatively retains the
                        // whole sub-batch. Newer responses may additionally
                        // identify rejected ownership collisions or malformed
                        // revisions; only those named queue rows consume a
                        // bounded retry while neighbors settle normally.
                        //
                        // Backward-compat: older cloud builds omit `skipped`
                        // entirely, in which case `skipped_count` is 0 and
                        // we fall through to the legacy mark-synced path.
                        match response.row_results_for(
                            entity_type,
                            batch_items.iter().map(|item| item.entity_id.clone()),
                        ) {
                            Ok(Some(rows)) => {
                                self.settle_personal_row_results(
                                    &batch_items,
                                    entity_type,
                                    &response.raw_body,
                                    rows,
                                    &mut outcome,
                                    &mut skip_errors,
                                );
                                continue;
                            }
                            Err(error) => {
                                let diagnostic = format!(
                                    "cloud returned invalid per-row results for {entity_type}: {error}; marking {} row(s) failed; server response: {}",
                                    batch_items.len(),
                                    response.raw_body
                                );
                                for item in &batch_items {
                                    let _ = self.queue.mark_failed(item.id, &diagnostic);
                                }
                                skip_errors.push(diagnostic);
                                continue;
                            }
                            Ok(None) => {}
                        }

                        let skipped_count = response.skipped_count_for(entity_type);
                        if let Err(error) = &skipped_count {
                            let diagnostic = format!(
                                "cloud returned an unrecognized skip signal for {entity_type}: {error}; marking {} row(s) failed; server response: {}",
                                batch_items.len(),
                                response.raw_body
                            );
                            warn!(
                                entity_type = entity_type,
                                error = error,
                                "Cloud response contained an unrecognized skip signal; marking sub-batch failed",
                            );
                            for item in &batch_items {
                                let _ = self.queue.mark_failed(item.id, &diagnostic);
                            }
                            skip_errors.push(diagnostic);
                            continue;
                        }
                        let skipped_count = skipped_count.unwrap_or_default();
                        if skipped_count > 0 {
                            let batch_size = batch_items.len();
                            if skipped_count > batch_size {
                                let diagnostic = format!(
                                    "cloud reported {skipped_count} skipped {entity_type} row(s) for a {batch_size}-row sub-batch; marking sub-batch failed; server response: {}",
                                    response.raw_body
                                );
                                for item in &batch_items {
                                    let _ = self.queue.mark_failed(item.id, &diagnostic);
                                }
                                skip_errors.push(diagnostic);
                                continue;
                            }
                            let itemized = response.itemized_failures_for(
                                entity_type,
                                skipped_count,
                                batch_items.iter().map(|item| item.entity_id.clone()),
                            );
                            let itemized = match itemized {
                                Ok(Some(failures)) => failures,
                                Ok(None) => {
                                    // Aggregate-only responses count benign LWW losses as
                                    // skipped but do not identify the row. Trust the server's
                                    // LWW semantics and consume the local rows as acknowledgements.
                                    let diagnostic = format!(
                                        "cloud skipped {skipped_count} of {batch_size} {entity_type} row(s); treating skips as LWW acknowledgements"
                                    );
                                    warn!(
                                        entity_type = entity_type,
                                        skipped = skipped_count,
                                        batch_size,
                                        "{diagnostic}"
                                    );
                                    for item in &batch_items {
                                        let _ = self.queue.mark_synced(item.id);
                                        outcome.synced += 1;
                                    }
                                    outcome.skipped_lww += skipped_count;
                                    continue;
                                }
                                Err(error) => {
                                    let diagnostic = format!(
                                        "cloud returned invalid itemized failures for {entity_type}: {error}; marking {batch_size} row(s) failed; server response: {}",
                                        response.raw_body
                                    );
                                    for item in &batch_items {
                                        let _ = self.queue.mark_failed(item.id, &diagnostic);
                                    }
                                    skip_errors.push(diagnostic);
                                    continue;
                                }
                            };

                            // The server supplied a complete identity mapping: only named
                            // rows are terminal failures; owned neighbors in the same request
                            // are safely removed from the local queue.
                            let mut failure_details = Vec::new();
                            for item in &batch_items {
                                if let Some(failure) = itemized.get(&item.entity_id) {
                                    let reason = match failure {
                                        PushItemizedFailure::Rejection(rejection) => {
                                            rejection.reason.as_str().to_string()
                                        }
                                        PushItemizedFailure::Invalid(invalid) => {
                                            format!(
                                                "{}: {}",
                                                invalid.reason.as_str(),
                                                invalid.detail
                                            )
                                        }
                                    };
                                    let diagnostic = match failure {
                                        PushItemizedFailure::Rejection(rejection) => format!(
                                            "permanent cloud rejection: reason={}; entity={entity_type}; id={}; existing_project={}",
                                            rejection.reason.as_str(),
                                            rejection.id,
                                            rejection.existing_canonical_id,
                                        ),
                                        PushItemizedFailure::Invalid(invalid) => format!(
                                            "cloud invalid {entity_type} {}: {} ({}); server response: {}",
                                            invalid.id,
                                            invalid.reason.as_str(),
                                            invalid.detail,
                                            response.raw_body
                                        ),
                                    };
                                    if matches!(failure, PushItemizedFailure::Rejection(rejection) if rejection.reason.is_permanent())
                                    {
                                        let _ = self.queue.park_failed(
                                            item.id,
                                            &diagnostic,
                                            self.config.max_retries,
                                        );
                                    } else {
                                        let _ = self.queue.mark_failed(item.id, &diagnostic);
                                    }
                                    let _ = self.queue.record_row_outcome(
                                        item.id,
                                        "rejected",
                                        Some(match failure {
                                            PushItemizedFailure::Rejection(rejection) => {
                                                rejection.reason.as_str()
                                            }
                                            PushItemizedFailure::Invalid(invalid) => {
                                                invalid.reason.as_str()
                                            }
                                        }),
                                    );
                                    failure_details.push(format!("{} ({reason})", item.entity_id));
                                } else {
                                    let _ = self.queue.mark_synced(item.id);
                                    outcome.synced += 1;
                                }
                            }
                            // Skips the server did not itemize are benign LWW
                            // losses: they were acknowledged above, so report
                            // them as kept-newer rather than silent successes.
                            outcome.skipped_lww +=
                                skipped_count.saturating_sub(failure_details.len());
                            if !failure_details.is_empty() {
                                skip_errors.push(format!(
                                    "cloud rejected {} of {} {entity_type} row(s): {}",
                                    failure_details.len(),
                                    batch_items.len(),
                                    failure_details.join(", ")
                                ));
                            }
                        } else {
                            for item in &batch_items {
                                let _ = self.queue.mark_synced(item.id);
                                outcome.synced += 1;
                            }
                        }
                    }
                    Err(e) => {
                        // Mark this sub-batch as failed but continue with others
                        for item in &batch_items {
                            let _ = self.queue.mark_failed(item.id, &e.to_string());
                        }
                        // If any sub-batch fails, report the error
                        return Err(e);
                    }
                }
            }
        }

        outcome.error = skip_errors.into_iter().next();

        // Send individual delete requests
        for item in deletes {
            let cas_id = item.entity_id.as_str();

            // Pull/apply writes deliberately bypass the syncing wrappers. A
            // remote restore can therefore recreate a task/entry while an old
            // local tombstone remains queued. Neutralize that exact tombstone
            // before making deletes functional; uncertainty fails closed.
            match self.queue.neutralize_delete_if_local_entity_exists(item) {
                Ok(true) => {
                    warn!(
                        entity_type = %item.entity_type,
                        entity_id = cas_id,
                        queue_id = item.id,
                        "Neutralized stale cloud delete because the entity still exists locally"
                    );
                    continue;
                }
                Ok(false) => {}
                Err(e) => {
                    let error = format!(
                        "Delete safety check failed for {} {cas_id}: {e}",
                        item.entity_type
                    );
                    let _ = self.queue.mark_failed(item.id, &error);
                    warn!(
                        entity_type = %item.entity_type,
                        entity_id = cas_id,
                        queue_id = item.id,
                        error = %e,
                        "Skipped cloud delete because local existence could not be verified"
                    );
                    continue;
                }
            }

            let delete_url = format!(
                "{}/api/sync/{}/{}?project_id={}",
                self.cloud_config.endpoint,
                item.entity_type.as_str(),
                cas_id,
                self.personal_push_project_id()?.replace('/', "%2F")
            );

            let response = ureq::delete(&delete_url)
                .timeout(self.config.timeout)
                .set("Authorization", &format!("Bearer {token}"))
                .call();

            match response {
                Ok(resp) if (200..300).contains(&resp.status()) => {
                    let _ = self.queue.mark_synced(item.id);
                    outcome.synced += 1;
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.into_string().unwrap_or_default();
                    let error = format!("Delete failed with status {status}: {body}");
                    let _ = self.queue.mark_failed(item.id, &error);
                    tracing::warn!("Delete {cas_id} failed with status {status}: {body}");
                }
                Err(ureq::Error::Status(404, _)) => {
                    // Already absent remotely is the desired final state.
                    let _ = self.queue.mark_synced(item.id);
                    outcome.synced += 1;
                }
                Err(ureq::Error::Status(status, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    let error = format!("Delete failed with status {status}: {body}");
                    let _ = self.queue.mark_failed(item.id, &error);
                    tracing::warn!("Delete {cas_id} failed with status {status}: {body}");
                }
                Err(ureq::Error::Transport(e)) => {
                    let error = format!("Delete failed: {e}");
                    let _ = self.queue.mark_failed(item.id, &error);
                    tracing::warn!("Delete {cas_id} failed: {e}");
                }
            }
        }

        Ok(outcome)
    }

    /// Split upsert entries into sub-batches that each stay under max_payload_bytes.
    /// Takes ownership of entries to avoid cloning serde_json::Value.
    pub(crate) fn split_into_sub_batches<'a>(
        &self,
        entries: Vec<(&'a QueuedSync, serde_json::Value)>,
    ) -> Vec<Vec<(&'a QueuedSync, serde_json::Value)>> {
        let max_bytes = self.config.max_payload_bytes;
        let overhead = 256;
        let mut batches = Vec::new();
        let mut current_batch: Vec<(&QueuedSync, serde_json::Value)> = Vec::new();
        let mut current_size = overhead;

        for (item, value) in entries {
            let item_size = item.payload.as_ref().map(|p| p.len()).unwrap_or(256);
            let item_total = item_size + 1;

            if !current_batch.is_empty() && current_size + item_total > max_bytes {
                batches.push(current_batch);
                current_batch = Vec::new();
                current_size = overhead;
            }

            current_batch.push((item, value));
            current_size += item_total;
        }

        if !current_batch.is_empty() {
            batches.push(current_batch);
        }

        batches
    }

    /// Exact personal-envelope batching. This measures the complete serialized
    /// request and its gzip bytes, including project/client metadata and JSON
    /// punctuation, instead of estimating from queue payload strings.
    fn split_personal_sub_batches<'a>(
        &self,
        entries: Vec<(&'a QueuedSync, serde_json::Value)>,
        entity_type: &str,
    ) -> Result<Vec<Vec<(&'a QueuedSync, serde_json::Value)>>, CasError> {
        let mut batches = Vec::new();
        let mut current: Vec<(&QueuedSync, serde_json::Value)> = Vec::new();

        for entry in entries {
            let mut candidate_values = current
                .iter()
                .map(|(_, value)| value.clone())
                .collect::<Vec<_>>();
            candidate_values.push(entry.1.clone());

            if !current.is_empty() && !self.personal_payload_fits(entity_type, candidate_values)? {
                batches.push(current);
                current = Vec::new();
            }
            current.push(entry);
        }

        if !current.is_empty() {
            batches.push(current);
        }
        Ok(batches)
    }

    fn personal_payload_fits(
        &self,
        entity_type: &str,
        values: Vec<serde_json::Value>,
    ) -> Result<bool, CasError> {
        let payload = self.build_personal_push_payload(entity_type, values)?;
        let json_bytes = serde_json::to_vec(&payload)
            .map_err(|e| CasError::Other(format!("JSON serialization failed: {e}")))?;
        let compressed = Self::gzip_json(&json_bytes)?;
        Ok(
            json_bytes.len() <= self.config.max_payload_bytes
                && compressed.len() <= 4 * 1024 * 1024,
        )
    }

    /// Shared builder for both personal push envelopes. The successor
    /// git-remote task can extend this seam without duplicating scope/version
    /// metadata across queued entities and directly supplied sessions.
    fn build_personal_push_payload(
        &self,
        entity_type: &str,
        values: Vec<serde_json::Value>,
    ) -> Result<serde_json::Map<String, serde_json::Value>, CasError> {
        self.build_push_payload_fields([(entity_type.to_string(), values)], None)
    }

    /// Build the shared envelope for knowledge pages. The caller passes the
    /// already-resolved active team so the page visibility decision and the
    /// optional top-level wire scope cannot consult different configuration
    /// fields. `None` preserves private knowledge pushes.
    pub(super) fn build_team_scoped_push_payload_fields<I>(
        &self,
        fields: I,
        team_id: Option<&str>,
    ) -> Result<serde_json::Map<String, serde_json::Value>, CasError>
    where
        I: IntoIterator<Item = (String, Vec<serde_json::Value>)>,
    {
        self.build_push_payload_fields(fields, team_id)
    }

    fn build_push_payload_fields<I>(
        &self,
        fields: I,
        team_id: Option<&str>,
    ) -> Result<serde_json::Map<String, serde_json::Value>, CasError>
    where
        I: IntoIterator<Item = (String, Vec<serde_json::Value>)>,
    {
        let mut payload = serde_json::Map::new();
        for (entity_type, values) in fields {
            payload.insert(entity_type, serde_json::Value::Array(values));
        }
        if let Some(team_id) = team_id {
            payload.insert("team_id".to_string(), serde_json::json!(team_id));
        }
        payload.insert(
            "project_canonical_id".to_string(),
            serde_json::json!(self.personal_push_project_id()?),
        );
        if let Some(git_remote) = &self.personal_push_git_remote {
            payload.insert("git_remote".to_string(), serde_json::json!(git_remote));
        }
        Self::insert_client_version(&mut payload);
        Ok(payload)
    }

    fn check_personal_payload_size(
        &self,
        uncompressed_bytes: usize,
        compressed_bytes: usize,
    ) -> Result<(), CasError> {
        if uncompressed_bytes > self.config.max_payload_bytes {
            return Err(CasError::Other(format!(
                "personal push payload is {uncompressed_bytes} bytes before gzip, exceeding the configured {}-byte limit",
                self.config.max_payload_bytes
            )));
        }
        const CLOUD_COMPRESSED_LIMIT: usize = 4 * 1024 * 1024;
        if compressed_bytes > CLOUD_COMPRESSED_LIMIT {
            return Err(CasError::Other(format!(
                "personal push payload is {compressed_bytes} gzip bytes, exceeding the cloud {CLOUD_COMPRESSED_LIMIT}-byte limit"
            )));
        }
        Ok(())
    }

    /// Gzip-compress a JSON payload.
    pub(crate) fn gzip_json(json_bytes: &[u8]) -> Result<Vec<u8>, CasError> {
        let mut encoder = GzEncoder::new(Vec::new(), Compression::fast());
        encoder
            .write_all(json_bytes)
            .map_err(|e| CasError::Other(format!("Gzip compression failed: {e}")))?;
        encoder
            .finish()
            .map_err(|e| CasError::Other(format!("Gzip finalize failed: {e}")))
    }

    /// Push a single sub-batch of upsert values with retry.
    ///
    /// Returns the parsed [`PushResponse`] on success. Callers should
    /// inspect `PushResponse::skipped_count_for(entity_type)` and treat any
    /// non-zero count as a signal that the server silently skipped some
    /// rows (see `PushResponse` docs and cas-f645 for the cross-project
    /// conflict contract). When the response body is empty or fails to
    /// parse (e.g. older cloud build returning a different shape), a
    /// `PushResponse::default()` is returned — `skipped` is then `None`,
    /// which `skipped_count_for` reports as `0`, preserving legacy
    /// "trust the 200" behavior.
    pub(super) fn push_sub_batch(
        &self,
        values: Vec<serde_json::Value>,
        entity_type: &str,
        token: &str,
    ) -> Result<PushResponse, CasError> {
        let payload = self.build_personal_push_payload(entity_type, values)?;
        self.push_personal_payload(payload, token)
    }

    /// Push an already-built personal envelope with the normal retry, gzip,
    /// and response parsing policy. Multi-key envelopes (knowledge pages plus
    /// tombstones) use the same transport contract as queued entities.
    pub(super) fn push_personal_payload(
        &self,
        payload: serde_json::Map<String, serde_json::Value>,
        token: &str,
    ) -> Result<PushResponse, CasError> {
        let push_url = format!("{}/api/sync/push", self.cloud_config.endpoint);

        // Serialize and compress the payload
        let json_bytes = serde_json::to_vec(&payload)
            .map_err(|e| CasError::Other(format!("JSON serialization failed: {e}")))?;
        let compressed = Self::gzip_json(&json_bytes)?;
        self.check_personal_payload_size(json_bytes.len(), compressed.len())?;

        let mut last_error = None;
        for attempt in 0..3 {
            if attempt > 0 {
                std::thread::sleep(self.config.backoff_duration(attempt as u32));
            }

            let response = ureq::post(&push_url)
                .timeout(self.config.timeout)
                .set("Authorization", &format!("Bearer {token}"))
                .set("Content-Type", "application/json")
                .set("Content-Encoding", "gzip")
                .send_bytes(&compressed);

            match response {
                Ok(resp) => {
                    if resp.status() == 200 || resp.status() == 201 {
                        // Read body so we can defensively inspect the
                        // server's `skipped` field. Treat parse failures
                        // (empty body, older cloud shape) as
                        // `PushResponse::default()` for backward compat —
                        // the 2xx status is the source of truth that the
                        // HTTP exchange itself succeeded.
                        let body = resp.into_string().unwrap_or_default();
                        let mut parsed: PushResponse = if body.is_empty() {
                            PushResponse::default()
                        } else {
                            serde_json::from_str(&body).unwrap_or_default()
                        };
                        parsed.raw_body = body;
                        return Ok(parsed);
                    } else {
                        let status = resp.status();
                        let body = resp.into_string().unwrap_or_default();
                        last_error = Some(CasError::Other(format!(
                            "Push failed with status {status}: {body}"
                        )));
                        if (400..500).contains(&status) {
                            break;
                        }
                    }
                }
                Err(ureq::Error::Status(code, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    last_error = Some(CasError::Other(format!(
                        "Push failed with status {code}: {body}"
                    )));
                    if (400..500).contains(&code) {
                        break;
                    }
                }
                Err(ureq::Error::Transport(e)) => {
                    last_error = Some(CasError::Other(format!("Network error: {e}")));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| CasError::Other("Push failed".to_string())))
    }

    /// Insert client version fields into a push payload.
    pub(crate) fn insert_client_version(payload: &mut serde_json::Map<String, serde_json::Value>) {
        payload.insert(
            "client_version".to_string(),
            serde_json::json!(env!("CARGO_PKG_VERSION")),
        );
        payload.insert(
            "client_build".to_string(),
            serde_json::json!(option_env!("CAS_GIT_HASH").unwrap_or("unknown")),
        );
    }
}

#[cfg(test)]
mod ephemeral_guard_root_tests {
    use std::sync::Arc;

    use super::super::{CloudSyncer, CloudSyncerConfig};
    use crate::cloud::CloudConfig;
    use crate::cloud::sync_queue::SyncQueue;

    fn syncer_for(root: &std::path::Path, pinned: bool) -> CloudSyncer {
        std::fs::create_dir_all(root).unwrap();
        if pinned {
            std::fs::write(
                root.join("config.toml"),
                "[project]\ncanonical_id = \"pinned-project\"\n",
            )
            .unwrap();
        }
        let queue = SyncQueue::open(root).unwrap();
        queue.init().unwrap();
        let mut cloud = CloudConfig::default();
        cloud.endpoint = "http://127.0.0.1:9".to_string();
        cloud.token = Some("test-token".to_string());
        CloudSyncer::new_for_project(
            Arc::new(queue),
            cloud,
            CloudSyncerConfig::default(),
            "pinned-project".to_string(),
            root,
        )
    }

    /// The guard must judge the root the syncer was built for. A pinned
    /// scratch root is durable; an unpinned one under /tmp is ephemeral —
    /// regardless of which project the test process itself runs inside.
    #[test]
    fn guard_classifies_the_syncers_own_root_not_the_process_root() {
        let base = tempfile::Builder::new()
            .prefix("cas-guard-root-")
            .tempdir_in("/tmp")
            .unwrap();
        let pinned = syncer_for(&base.path().join("pinned").join(".cas"), true);
        assert!(
            pinned.ephemeral_project_refusal().is_none(),
            "a pinned root is durable wherever it lives"
        );
        let unpinned = syncer_for(&base.path().join("scratch").join(".cas"), false);
        let refusal = unpinned.ephemeral_project_refusal();
        assert!(
            refusal.as_deref().is_some_and(|r| r.contains("/tmp")),
            "an unpinned /tmp root must be refused by its own path, got {refusal:?}"
        );
    }
}
