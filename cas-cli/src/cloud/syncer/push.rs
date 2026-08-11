use chrono::Utc;
use flate2::Compression;
use flate2::write::GzEncoder;
use std::io::Write;
use std::time::Instant;
use tracing::warn;

use crate::cloud::syncer::{
    CloudSyncer, PushItemizedFailure, PushPlan, PushResponse, PushScope, SyncResult,
};
use crate::cloud::{QueuedSync, SyncOperation};
use crate::error::CasError;
use crate::types::Session;

impl CloudSyncer {
    pub fn push(&self) -> Result<SyncResult, CasError> {
        self.push_scoped(PushScope::All)
    }

    /// Describe the exact next queue batch without mutating it.
    pub fn plan_push(&self, scope: PushScope) -> Result<PushPlan, CasError> {
        let batch_limit = self.config.batch_size.max(1);
        let items = self.queue.pending_for_entity_type(
            scope.entity_type(),
            batch_limit,
            self.config.max_retries,
        )?;
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
            batch_limit,
            batch_limit_reached: items.len() == batch_limit,
        })
    }

    /// Push only queue rows selected by `scope`.
    pub fn push_scoped(&self, scope: PushScope) -> Result<SyncResult, CasError> {
        self.push_scoped_with_sessions(scope, &[])
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
        let mut result = SyncResult::default();
        let start = Instant::now();

        if !self.is_available() {
            return Ok(result);
        }

        let batch_limit = self.config.batch_size.max(1);
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

        // Check if there's anything to push
        if pending.is_empty() && sessions.is_empty() {
            result.duration_ms = start.elapsed().as_millis() as u64;
            return Ok(result);
        }

        let token = self
            .cloud_config
            .token
            .as_ref()
            .ok_or_else(|| CasError::Other("Not logged in".to_string()))?;

        // Push each entity type
        if !pending.entries.is_empty() {
            match self.push_batch(&pending.entries, "entries", token) {
                Ok(count) => result.pushed_entries = count,
                Err(e) => {
                    result.errors.push(format!("Entry push failed: {e}"));
                }
            }
        }

        if !pending.tasks.is_empty() {
            match self.push_batch(&pending.tasks, "tasks", token) {
                Ok(count) => result.pushed_tasks = count,
                Err(e) => {
                    result.errors.push(format!("Task push failed: {e}"));
                }
            }
        }

        if !pending.rules.is_empty() {
            match self.push_batch(&pending.rules, "rules", token) {
                Ok(count) => result.pushed_rules = count,
                Err(e) => {
                    result.errors.push(format!("Rule push failed: {e}"));
                }
            }
        }

        if !pending.skills.is_empty() {
            match self.push_batch(&pending.skills, "skills", token) {
                Ok(count) => result.pushed_skills = count,
                Err(e) => {
                    result.errors.push(format!("Skill push failed: {e}"));
                }
            }
        }

        // Push sessions (queued or directly passed)
        if !pending.sessions.is_empty() {
            match self.push_batch(&pending.sessions, "sessions", token) {
                Ok(count) => result.pushed_sessions = count,
                Err(e) => {
                    result.errors.push(format!("Session push failed: {e}"));
                }
            }
        } else if !sessions.is_empty() {
            // Fallback to directly-passed sessions
            match self.push_sessions(sessions, token) {
                Ok(count) => result.pushed_sessions = count,
                Err(e) => {
                    result.errors.push(format!("Session push failed: {e}"));
                }
            }
        }

        // Push verifications
        if !pending.verifications.is_empty() {
            match self.push_batch(&pending.verifications, "verifications", token) {
                Ok(count) => result.pushed_verifications = count,
                Err(e) => {
                    result.errors.push(format!("Verification push failed: {e}"));
                }
            }
        }

        // Push events
        if !pending.events.is_empty() {
            match self.push_batch(&pending.events, "events", token) {
                Ok(count) => result.pushed_events = count,
                Err(e) => {
                    result.errors.push(format!("Event push failed: {e}"));
                }
            }
        }

        // Push prompts
        if !pending.prompts.is_empty() {
            match self.push_batch(&pending.prompts, "prompts", token) {
                Ok(count) => result.pushed_prompts = count,
                Err(e) => {
                    result.errors.push(format!("Prompt push failed: {e}"));
                }
            }
        }

        // Push file changes
        if !pending.file_changes.is_empty() {
            match self.push_batch(&pending.file_changes, "file_changes", token) {
                Ok(count) => result.pushed_file_changes = count,
                Err(e) => {
                    result.errors.push(format!("FileChange push failed: {e}"));
                }
            }
        }

        // Push commit links
        if !pending.commit_links.is_empty() {
            match self.push_batch(&pending.commit_links, "commit_links", token) {
                Ok(count) => result.pushed_commit_links = count,
                Err(e) => {
                    result.errors.push(format!("CommitLink push failed: {e}"));
                }
            }
        }

        // Push agents
        if !pending.agents.is_empty() {
            match self.push_batch(&pending.agents, "agents", token) {
                Ok(count) => result.pushed_agents = count,
                Err(e) => {
                    result.errors.push(format!("Agent push failed: {e}"));
                }
            }
        }

        // Push worktrees
        if !pending.worktrees.is_empty() {
            match self.push_batch(&pending.worktrees, "worktrees", token) {
                Ok(count) => result.pushed_worktrees = count,
                Err(e) => {
                    result.errors.push(format!("Worktree push failed: {e}"));
                }
            }
        }

        // Update last push timestamp
        let _ = self
            .queue
            .set_metadata("last_push_at", &Utc::now().to_rfc3339());

        result.duration_ms = start.elapsed().as_millis() as u64;
        Ok(result)
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

    fn push_batch(
        &self,
        items: &[QueuedSync],
        entity_type: &str,
        token: &str,
    ) -> Result<usize, CasError> {
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
                    Ok(v) => upsert_entries.push((*item, v)),
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

        let mut synced_count = 0;
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
                            let itemized = response.itemized_failures_for(
                                entity_type,
                                skipped_count,
                                batch_items.iter().map(|item| item.entity_id.clone()),
                            );
                            let itemized = match itemized {
                                Ok(Some(failures)) => failures,
                                Ok(None) => {
                                    // Aggregate-only servers cannot identify individual rows.
                                    // Preserve cas-607a's conservative whole-batch retry path.
                                    let diagnostic = format!(
                                        "cloud skipped {skipped_count} of {batch_size} {entity_type} row(s); marking the indistinguishable sub-batch failed; server response: {}",
                                        response.raw_body
                                    );
                                    for item in &batch_items {
                                        let _ = self.queue.mark_failed(item.id, &diagnostic);
                                    }
                                    skip_errors.push(diagnostic);
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
                            for item in &batch_items {
                                if let Some(failure) = itemized.get(&item.entity_id) {
                                    let diagnostic = match failure {
                                        PushItemizedFailure::Rejection(rejection) => format!(
                                            "cloud rejected {entity_type} {}: {} (existing canonical project: {}); server response: {}",
                                            rejection.id,
                                            rejection.reason.as_str(),
                                            rejection.existing_canonical_id,
                                            response.raw_body
                                        ),
                                        PushItemizedFailure::Invalid(invalid) => format!(
                                            "cloud invalid {entity_type} {}: {} ({}); server response: {}",
                                            invalid.id,
                                            invalid.reason.as_str(),
                                            invalid.detail,
                                            response.raw_body
                                        ),
                                    };
                                    let _ = self.queue.mark_failed(item.id, &diagnostic);
                                    skip_errors.push(diagnostic);
                                } else {
                                    let _ = self.queue.mark_synced(item.id);
                                    synced_count += 1;
                                }
                            }
                        } else {
                            for item in &batch_items {
                                let _ = self.queue.mark_synced(item.id);
                                synced_count += 1;
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

        if let Some(error) = skip_errors.into_iter().next() {
            return Err(CasError::Other(error));
        }

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
                "{}/api/sync/{}/{}",
                self.cloud_config.endpoint,
                item.entity_type.as_str(),
                cas_id
            );

            let response = ureq::delete(&delete_url)
                .timeout(self.config.timeout)
                .set("Authorization", &format!("Bearer {token}"))
                .call();

            match response {
                Ok(resp) if (200..300).contains(&resp.status()) => {
                    let _ = self.queue.mark_synced(item.id);
                    synced_count += 1;
                }
                Ok(resp) => {
                    let status = resp.status();
                    let body = resp.into_string().unwrap_or_default();
                    let error = format!("Delete failed with status {status}: {body}");
                    let _ = self.queue.mark_failed(item.id, &error);
                    eprintln!("Delete {cas_id} failed with status {status}: {body}");
                }
                Err(ureq::Error::Status(404, _)) => {
                    // Already absent remotely is the desired final state.
                    let _ = self.queue.mark_synced(item.id);
                    synced_count += 1;
                }
                Err(ureq::Error::Status(status, resp)) => {
                    let body = resp.into_string().unwrap_or_default();
                    let error = format!("Delete failed with status {status}: {body}");
                    let _ = self.queue.mark_failed(item.id, &error);
                    eprintln!("Delete {cas_id} failed with status {status}: {body}");
                }
                Err(ureq::Error::Transport(e)) => {
                    let error = format!("Delete failed: {e}");
                    let _ = self.queue.mark_failed(item.id, &error);
                    eprintln!("Delete {cas_id} failed: {e}");
                }
            }
        }

        Ok(synced_count)
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
